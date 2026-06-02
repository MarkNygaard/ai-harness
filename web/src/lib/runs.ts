/**
 * Data layer for the control-plane **runs** API.
 *
 * - `useRuns()`        — poll `GET /runs` for the list view.
 * - `useRunDetail(id)` — `GET /runs/{id}` (404s until a run finishes, since runs
 *                        persist on completion; we poll so it appears when ready).
 * - `useCreateRun()`   — `POST /runs`.
 * - `useRunStream()`   — low-level SSE reader of `GET /runs/{id}/stream`.
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
  NodeView,
  RunDetail,
  RunEvent,
  RunStatus,
  RunSummary,
} from "@/types/run";

const EMPTY_USAGE = { input: null, output: null, cache_read: null, cache_write: null };

export function useRuns() {
  return useQuery<RunSummary[], Error>({
    queryKey: ["runs"],
    queryFn: ({ signal }) => apiJson<RunSummary[]>("/runs", { signal }),
    refetchInterval: 5_000,
  });
}

export function useRunDetail(id: string | null, live: boolean) {
  return useQuery<RunDetail, Error>({
    queryKey: ["run", id],
    queryFn: ({ signal }) => apiJson<RunDetail>(`/runs/${id}`, { signal }),
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
      apiJson<CreateRunResponse>("/runs", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
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
        const resp = await apiFetch(`/runs/${id}/stream`, {
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
            const dataLine = part.split("\n").find((l) => l.startsWith("data:"));
            if (!dataLine) continue;
            try {
              onEventRef.current(JSON.parse(dataLine.slice(5).trim()) as RunEvent);
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

type LiveAction = { type: "event"; event: RunEvent; now: string } | { type: "reset" };

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
      const prev = state.nodes[event.node_id] ?? seedNode({ id: event.node_id, depends_on: [] });
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

/** Build render-ready [`NodeView`]s from a persisted [`RunDetail`]. Pure. */
export function nodesFromDetail(detail: RunDetail): NodeView[] {
  const deps = new Map(detail.graph.map((g) => [g.id, g.depends_on]));
  return detail.nodes.map((n) => ({
    id: n.node_id,
    depends_on: deps.get(n.node_id) ?? [],
    status: n.status,
    provider: n.provider,
    model: n.model,
    iterations: n.iterations,
    usage: {
      input: n.input_tokens,
      output: n.output_tokens,
      cache_read: n.cache_read,
      cache_write: n.cache_write,
    },
    note: n.note,
    output: n.output,
    started_at: n.started_at,
    ended_at: n.ended_at,
  }));
}

export interface RunView {
  workflow: string | null;
  status: RunStatus;
  nodes: NodeView[];
  /** True while the run is executing (driven by SSE, not yet persisted). */
  live: boolean;
}

/**
 * The combined run view: persisted detail is authoritative once present;
 * otherwise the live SSE accumulator drives the graph. Handles both a
 * freshly-submitted run (live → finishes → detail loads) and a historical run
 * (stream 404s → detail loads immediately).
 */
export function useRunView(id: string | null): RunView {
  const [state, dispatch] = useReducer(liveReducer, {
    workflow: null,
    status: "running",
    nodes: {},
    order: [],
  });

  // Reset the accumulator when the run id changes.
  useEffect(() => {
    dispatch({ type: "reset" });
  }, [id]);

  const runFinished = state.status !== "running";
  const detail = useRunDetail(id, !runFinished);

  const handleEvent = useCallback((event: RunEvent) => {
    dispatch({ type: "event", event, now: new Date().toISOString() });
  }, []);
  useRunStream(id, handleEvent);

  return useMemo<RunView>(() => {
    if (detail.data) {
      return {
        workflow: detail.data.workflow_name,
        status: detail.data.status,
        nodes: nodesFromDetail(detail.data),
        live: false,
      };
    }
    const nodes = state.order.length
      ? state.order.map((nid) => state.nodes[nid])
      : Object.values(state.nodes);
    return {
      workflow: state.workflow,
      status: state.status,
      nodes,
      live: state.status === "running",
    };
  }, [detail.data, state]);
}
