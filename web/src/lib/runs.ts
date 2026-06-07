/**
 * Data layer for the control-plane **runs** API.
 *
 * - `useRuns()`        — poll `GET /api/runs` for the list view.
 * - `useRunDetail(id)` — `GET /api/runs/{id}` (404s until a run finishes, since
 *                        runs persist on completion; we poll so it appears).
 * - `useCreateRun()`   — `POST /api/runs`.
 * - `useRunStream()`   — low-level SSE reader of `GET /api/runs/{id}/stream`.
 * - `useRunView(id)`   — the combined, render-ready view: persisted detail when
 *                        available, otherwise the live accumulator fed by SSE.
 */
import { useCallback, useEffect, useMemo, useReducer, useRef } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./api";
import type {
  CreateRunRequest,
  CreateRunResponse,
  NodeMeta,
  NodeStatus,
  NodeView,
  RunDetail,
  RunEvent,
  RunStatus,
  RunSummary,
} from "@/types/run";

const EMPTY_USAGE = {
  input: null,
  output: null,
  cache_read: null,
  cache_write: null,
};

/**
 * How far along a node is, for merging the live stream with persisted state.
 * pending < running < terminal. Equal rank → prefer the persisted row (it
 * carries the finished output/usage the live event may lack).
 */
const STATUS_RANK: Record<NodeStatus, number> = {
  pending: 0,
  running: 1,
  success: 2,
  failed: 2,
  skipped: 2,
  cancelled: 2,
};

export function useRuns() {
  return useQuery<RunSummary[], Error>({
    queryKey: ["runs"],
    queryFn: ({ signal }) => apiJson<RunSummary[]>("/api/runs", { signal }),
    refetchInterval: 5_000,
  });
}

export function useRunDetail(id: string | null, live: boolean) {
  return useQuery<RunDetail, Error>({
    queryKey: ["run", id],
    queryFn: ({ signal }) => apiJson<RunDetail>(`/api/runs/${id}`, { signal }),
    enabled: !!id,
    // While a run may still be executing, poll so the persisted detail appears
    // as soon as it finishes; a missing run simply 404s until then.
    retry: false,
    refetchInterval: live ? 4_000 : false,
  });
}

export function useCreateRun() {
  const qc = useQueryClient();
  return useMutation<CreateRunResponse, Error, CreateRunRequest>({
    mutationFn: (body) =>
      apiJson<CreateRunResponse>("/api/runs", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["runs"] }),
  });
}

/** `POST /api/runs/{id}/cancel` — stop a running run. */
export function useCancelRun() {
  const qc = useQueryClient();
  return useMutation<void, Error, string>({
    mutationFn: async (id) => {
      await apiFetch(`/api/runs/${id}/cancel`, { method: "POST" });
    },
    onSuccess: (_data, id) => {
      qc.invalidateQueries({ queryKey: ["runs"] });
      qc.invalidateQueries({ queryKey: ["run", id] });
    },
  });
}

/** `DELETE /api/runs/{id}` — remove a run from the list. */
export function useDeleteRun() {
  const qc = useQueryClient();
  return useMutation<void, Error, string>({
    mutationFn: async (id) => {
      await apiFetch(`/api/runs/${id}`, { method: "DELETE" });
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ["runs"] }),
  });
}

/**
 * Read `GET /runs/{id}/stream` as SSE, parsing each `data:` line into a
 * [`RunEvent`] and invoking `onEvent`. `onClose(streaming)` fires once when the
 * stream ends — `streaming=false` means the run was not live (already finished
 * or unknown), the signal to fall back to the persisted detail.
 */
export function useRunStream(
  id: string | null,
  onEvent: (event: RunEvent) => void,
  onClose?: (wasStreaming: boolean) => void,
): void {
  const onEventRef = useRef(onEvent);
  const onCloseRef = useRef(onClose);
  useEffect(() => {
    onEventRef.current = onEvent;
    onCloseRef.current = onClose;
  });

  useEffect(() => {
    if (!id) return;
    const controller = new AbortController();
    let streaming = false;

    (async () => {
      try {
        const resp = await apiFetch(`/api/runs/${id}/stream`, {
          signal: controller.signal,
          headers: { Accept: "text/event-stream" },
        });
        streaming = true;
        if (!resp.body) return;
        const reader = resp.body.getReader();
        const decoder = new TextDecoder();
        let buf = "";
        while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          buf += decoder.decode(value, { stream: true });
          const parts = buf.split("\n\n");
          buf = parts.pop() ?? "";
          for (const part of parts) {
            const dataLine = part
              .split("\n")
              .find((l) => l.startsWith("data:"));
            if (!dataLine) continue;
            try {
              onEventRef.current(
                JSON.parse(dataLine.slice(5).trim()) as RunEvent,
              );
            } catch {
              // malformed SSE line — skip
            }
          }
        }
      } catch (err) {
        // A 404 (run not streaming) surfaces as ApiError; treat as "not live".
        if (err instanceof Error && err.name === "AbortError") return;
      } finally {
        if (!controller.signal.aborted) onCloseRef.current?.(streaming);
      }
    })();

    return () => controller.abort();
  }, [id]);
}

// ── Live accumulator ───────────────────────────────────────────────────────

interface LiveState {
  workflow: string | null;
  status: RunStatus;
  nodes: Record<string, NodeView>;
  order: string[];
}

type LiveAction =
  | { type: "event"; event: RunEvent; now: string }
  | { type: "reset" };

function seedNode(meta: NodeMeta): NodeView {
  return {
    id: meta.id,
    depends_on: meta.depends_on,
    status: "pending",
    provider: null,
    model: null,
    iterations: 0,
    usage: { ...EMPTY_USAGE },
    note: null,
    output: "",
    started_at: null,
    ended_at: null,
    category: meta.category ?? null,
    artifact: meta.artifact ?? null,
    artifact_content: null,
  };
}

/** Reduce one live [`RunEvent`] into the accumulated view. Pure (now injected). */
export function liveReducer(state: LiveState, action: LiveAction): LiveState {
  if (action.type === "reset") {
    return { workflow: null, status: "running", nodes: {}, order: [] };
  }
  const { event, now } = action;
  switch (event.type) {
    case "run_started": {
      const nodes: Record<string, NodeView> = {};
      for (const meta of event.nodes) nodes[meta.id] = seedNode(meta);
      return {
        workflow: event.workflow,
        status: "running",
        nodes,
        order: event.nodes.map((n) => n.id),
      };
    }
    case "node_started": {
      const prev =
        state.nodes[event.node_id] ??
        seedNode({ id: event.node_id, depends_on: [] });
      return {
        ...state,
        nodes: {
          ...state.nodes,
          [event.node_id]: {
            ...prev,
            status: "running",
            provider: event.provider,
            model: event.model,
            started_at: prev.started_at ?? now,
          },
        },
      };
    }
    case "node_finished": {
      const n = event.node;
      const prev = state.nodes[n.id] ?? seedNode({ id: n.id, depends_on: [] });
      return {
        ...state,
        nodes: {
          ...state.nodes,
          [n.id]: {
            ...prev,
            status: n.status,
            provider: n.provider ?? prev.provider,
            model: n.model ?? prev.model,
            iterations: n.iterations,
            usage: n.usage,
            note: n.note,
            output: n.output,
            started_at: n.started_at ?? prev.started_at,
            ended_at: n.ended_at ?? now,
            artifact_content: n.artifact_content ?? prev.artifact_content,
          },
        },
      };
    }
    case "run_finished":
      return { ...state, status: event.status };
    default:
      return state;
  }
}

/**
 * Build render-ready [`NodeView`]s from a persisted [`RunDetail`]. Pure.
 *
 * Seeds from the full DAG **topology** (`detail.graph`) so every declared step
 * shows — not-yet-run ones as `pending` — then overlays each persisted node's
 * status/usage/output. This makes a still-running run (or one triggered
 * out-of-band, where this client never streamed the `run_started` event) render
 * the whole graph on load instead of only finished steps. Falls back to the
 * persisted node rows when no topology is stored (older runs).
 */
export function nodesFromDetail(detail: RunDetail): NodeView[] {
  const byId = new Map(detail.nodes.map((n) => [n.node_id, n]));
  const skeleton = detail.graph.length
    ? detail.graph.map((g) => ({
        id: g.id,
        depends_on: g.depends_on,
        category: g.category ?? null,
        artifact: g.artifact ?? null,
      }))
    : detail.nodes.map((n) => ({
        id: n.node_id,
        depends_on: [] as string[],
        category: null,
        artifact: null,
      }));
  return skeleton.map(({ id, depends_on, category, artifact }) => {
    const n = byId.get(id);
    return {
      id,
      depends_on,
      status: n?.status ?? "pending",
      provider: n?.provider ?? null,
      model: n?.model ?? null,
      iterations: n?.iterations ?? 0,
      usage: {
        input: n?.input_tokens ?? null,
        output: n?.output_tokens ?? null,
        cache_read: n?.cache_read ?? null,
        cache_write: n?.cache_write ?? null,
      },
      note: n?.note ?? null,
      output: n?.output ?? "",
      started_at: n?.started_at ?? null,
      ended_at: n?.ended_at ?? null,
      category,
      artifact: artifact ?? null,
      artifact_content: n?.artifact_content ?? null,
    };
  });
}

export interface RunView {
  workflow: string | null;
  /** The task title (persisted); null until/unless set. */
  title: string | null;
  /** The task spec (persisted); null until/unless set. */
  description: string | null;
  status: RunStatus;
  nodes: NodeView[];
  /** True while the run is executing (driven by SSE, not yet persisted). */
  live: boolean;
  /** Project the run executed in; null for older/CLI runs. */
  project: string | null;
  /** When the run row was last recorded (ISO); null if not yet persisted. */
  recordedAt: string | null;
}

/**
 * The combined run view. The full DAG topology (from the persisted detail, which
 * exists from run-start) is the skeleton; live SSE state and persisted node rows
 * are merged onto it per node. The graph therefore always shows every step and
 * only updates states in place — it never collapses to the subset the live
 * stream happened to observe (e.g. when the page subscribed mid-run).
 */
export function useRunView(id: string | null): RunView {
  const [state, dispatch] = useReducer(liveReducer, {
    workflow: null,
    status: "running",
    nodes: {},
    order: [],
  });
  const qc = useQueryClient();
  const wasRunning = useRef(true);
  // Reset the accumulator when the run id changes.
  useEffect(() => {
    dispatch({ type: "reset" });
    wasRunning.current = true;
  }, [id]);
  const handleEvent = useCallback((event: RunEvent) => {
    dispatch({ type: "event", event, now: new Date().toISOString() });
  }, []);
  useRunStream(id, handleEvent);
  // When the live stream reports the run is finished, invalidate the cached
  // detail so the UI picks up the final persisted state (including artifact
  // content captured after the workflow completes but before record_run).
  useEffect(() => {
    if (wasRunning.current && state.status !== "running" && id) {
      wasRunning.current = false;
      qc.invalidateQueries({ queryKey: ["run", id] });
    }
    if (state.status === "running") {
      wasRunning.current = true;
    }
  }, [state.status, id, qc]);
  // Keep polling the persisted detail until *either* the live stream or the
  // persisted row reports a terminal status (covers a refresh after finish).
  return useRunViewMemo(state, id);
}

function useRunViewMemo(state: LiveState, id: string | null): RunView {
  const liveTerminal = state.status !== "running";
  const detail = useRunDetail(id, !liveTerminal);

  return useMemo<RunView>(() => {
    const d = detail.data;
    // Persisted view always carries the full topology (unstarted steps included).
    const persisted = d ? nodesFromDetail(d) : [];
    const persistedById = new Map(persisted.map((n) => [n.id, n]));

    // Render order: the full topology when we have it, else the live order.
    const order = persisted.length
      ? persisted.map((n) => n.id)
      : state.order.length
        ? state.order
        : Object.keys(state.nodes);

    // Merge per node: keep whichever source is furthest along. Live carries the
    // realtime `running` state; the persisted row carries finished output/usage.
    // Edges always come from the persisted topology (live may have missed
    // `run_started` and so lack depends_on).
    const nodes: NodeView[] = order.map((nid) => {
      const p = persistedById.get(nid);
      const l = state.nodes[nid];
      const chosen =
        p && l
          ? STATUS_RANK[l.status] > STATUS_RANK[p.status]
            ? l
            : p
          : (p ?? l ?? seedNode({ id: nid, depends_on: [] }));
      const depends_on = p?.depends_on ?? chosen.depends_on;
      return depends_on === chosen.depends_on
        ? chosen
        : { ...chosen, depends_on };
    });

    const status = liveTerminal ? state.status : (d?.status ?? state.status);
    return {
      workflow: d?.workflow_name ?? state.workflow ?? null,
      title: d?.title ?? null,
      description: d?.description ?? null,
      status,
      nodes,
      live: status === "running",
      project: d?.project ?? null,
      recordedAt: d?.recorded_at ?? null,
    };
  }, [detail.data, state, liveTerminal]);
}
