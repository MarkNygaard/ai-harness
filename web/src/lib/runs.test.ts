import { describe, expect, it } from "vitest";
import { liveReducer, nodesFromDetail } from "./runs";
import type { RunDetail, RunEvent } from "@/types/run";

const NOW = "2026-01-01T00:00:10.000Z";

function reduce(events: RunEvent[]) {
  let state = liveReducer(undefined as never, { type: "reset" });
  for (const event of events)
    state = liveReducer(state, { type: "event", event, now: NOW });
  return state;
}

describe("liveReducer", () => {
  it("seeds topology from run_started", () => {
    const state = reduce([
      {
        type: "run_started",
        workflow: "demo",
        total_nodes: 2,
        nodes: [
          { id: "a", depends_on: [] },
          { id: "b", depends_on: ["a"] },
        ],
      },
    ]);
    expect(state.workflow).toBe("demo");
    expect(state.order).toEqual(["a", "b"]);
    expect(state.nodes.a.status).toBe("pending");
    expect(state.nodes.b.depends_on).toEqual(["a"]);
    expect(state.nodes.a.artifact).toBeNull();
    expect(state.nodes.b.artifact).toBeNull();
  });
  it("seeds artifact from run_started and propagates artifact_content on node_finished", () => {
    const state = reduce([
      {
        type: "run_started",
        workflow: "demo",
        total_nodes: 1,
        nodes: [{ id: "a", depends_on: [], artifact: "exploration.md" }],
      },
      {
        type: "node_finished",
        node: {
          id: "a",
          status: "success",
          provider: null,
          model: null,
          output: "ok",
          usage: { input: 0, output: 0, cache_read: null, cache_write: null },
          iterations: 1,
          converged: null,
          note: null,
          artifact_content: "# Explore\nsample",
        },
      },
    ]);
    expect(state.nodes.a.artifact).toBe("exploration.md");
    expect(state.nodes.a.artifact_content).toBe("# Explore\nsample");
  });

  it("marks a node running then merges its finished record", () => {
    const state = reduce([
      {
        type: "run_started",
        workflow: "demo",
        total_nodes: 1,
        nodes: [{ id: "a", depends_on: [] }],
      },
      {
        type: "node_started",
        node_id: "a",
        provider: "claude",
        model: "sonnet",
      },
      {
        type: "node_finished",
        node: {
          id: "a",
          status: "success",
          provider: "claude",
          model: "sonnet",
          output: "ok",
          usage: {
            input: 100,
            output: 20,
            cache_read: null,
            cache_write: null,
          },
          iterations: 1,
          converged: null,
          note: null,
          started_at: "2026-01-01T00:00:00.000Z",
          ended_at: "2026-01-01T00:00:05.000Z",
        },
      },
    ]);
    expect(state.nodes.a.status).toBe("success");
    expect(state.nodes.a.model).toBe("sonnet");
    expect(state.nodes.a.usage.input).toBe(100);
    expect(state.nodes.a.ended_at).toBe("2026-01-01T00:00:05.000Z");
  });

  it("accumulates a deduped activity feed and clears it when the node finishes", () => {
    const base: RunEvent[] = [
      {
        type: "run_started",
        workflow: "demo",
        total_nodes: 1,
        nodes: [{ id: "a", depends_on: [] }],
      },
      {
        type: "node_started",
        node_id: "a",
        provider: "claude",
        model: "sonnet",
      },
      { type: "node_progress", node_id: "a", activity: "reading files" },
      { type: "node_progress", node_id: "a", activity: "reading files" },
      { type: "node_progress", node_id: "a", activity: "2/11 tasks" },
    ];
    const running = reduce(base);
    // Latest line drives the node glance; the feed keeps the distinct history.
    expect(running.nodes.a.activity).toBe("2/11 tasks");
    expect(running.nodes.a.activityLog).toEqual([
      "reading files",
      "2/11 tasks",
    ]);

    const finished = reduce([
      ...base,
      {
        type: "node_finished",
        node: {
          id: "a",
          status: "success",
          provider: "claude",
          model: "sonnet",
          output: "ok",
          usage: { input: 1, output: 1, cache_read: null, cache_write: null },
          iterations: 1,
          converged: null,
          note: null,
          started_at: "2026-01-01T00:00:00.000Z",
          ended_at: "2026-01-01T00:00:05.000Z",
        },
      },
    ]);
    expect(finished.nodes.a.activity).toBeNull();
    expect(finished.nodes.a.activityLog).toEqual([]);
  });

  it("parses a sticky live-progress badge from 📋 task and 🔁 loop markers", () => {
    const state = reduce([
      {
        type: "run_started",
        workflow: "demo",
        total_nodes: 1,
        nodes: [{ id: "a", depends_on: [] }],
      },
      { type: "node_started", node_id: "a", provider: "pi", model: "kimi" },
      { type: "node_progress", node_id: "a", activity: "📋 3/13 wiring it" },
      // A non-marker line keeps the badge (sticky) but updates the latest line.
      { type: "node_progress", node_id: "a", activity: "⚙ bash" },
      { type: "node_progress", node_id: "a", activity: "📋 4/13 next task" },
    ]);
    expect(state.nodes.a.liveProgress).toEqual({
      done: 4,
      total: 13,
      kind: "task",
    });
    expect(state.nodes.a.activity).toBe("📋 4/13 next task");

    // A loop marker is tagged kind:"loop" (total is a max, stops early).
    const loop = reduce([
      {
        type: "run_started",
        workflow: "demo",
        total_nodes: 1,
        nodes: [{ id: "r", depends_on: [] }],
      },
      { type: "node_started", node_id: "r", provider: "pi", model: "kimi" },
      { type: "node_progress", node_id: "r", activity: "🔁 2/5" },
    ]);
    expect(loop.nodes.r.liveProgress).toEqual({
      done: 2,
      total: 5,
      kind: "loop",
    });

    // Cleared when the node starts again (e.g. a fresh run reuses the id).
    const restarted = liveReducer(state, {
      type: "event",
      event: {
        type: "node_started",
        node_id: "a",
        provider: "pi",
        model: "kimi",
      },
      now: NOW,
    });
    expect(restarted.nodes.a.liveProgress).toBeNull();
  });

  it("records terminal run status", () => {
    const state = reduce([
      { type: "run_started", workflow: "d", total_nodes: 0, nodes: [] },
      { type: "run_finished", status: "completed" },
    ]);
    expect(state.status).toBe("completed");
  });

  it("stamps started_at on node_started when the event has no timestamp", () => {
    const state = reduce([
      {
        type: "run_started",
        workflow: "d",
        total_nodes: 1,
        nodes: [{ id: "a", depends_on: [] }],
      },
      { type: "node_started", node_id: "a", provider: null, model: null },
    ]);
    expect(state.nodes.a.started_at).toBe(NOW);
  });
});

describe("nodesFromDetail", () => {
  it("merges topology edges with persisted node rows", () => {
    const detail: RunDetail = {
      id: "r1",
      workflow_name: "demo",
      title: "Demo task",
      description: null,
      status: "completed",
      project: null,
      node_count: 2,
      recorded_at: NOW,
      started_at: null,
      ended_at: null,
      ab_pair_id: null,
      ab_arm: null,
      ab_label: null,
      graph: [
        { id: "build", depends_on: [] },
        { id: "review", depends_on: ["build"] },
      ],
      nodes: [
        {
          node_id: "build",
          ordinal: 0,
          status: "success",
          provider: "claude",
          model: "sonnet",
          output: "",
          iterations: 1,
          converged: null,
          note: null,
          input_tokens: 100,
          output_tokens: 20,
          cache_read: null,
          cache_write: null,
          started_at: null,
          ended_at: null,
          artifact_content: null,
        },
        {
          node_id: "review",
          ordinal: 1,
          status: "skipped",
          provider: null,
          model: null,
          output: "",
          iterations: 0,
          converged: null,
          note: "dep failed",
          input_tokens: null,
          output_tokens: null,
          cache_read: null,
          cache_write: null,
          started_at: null,
          ended_at: null,
          artifact_content: null,
        },
      ],
    };
    const views = nodesFromDetail(detail);
    expect(views.map((v) => v.id)).toEqual(["build", "review"]);
    expect(views[1].depends_on).toEqual(["build"]);
    expect(views[0].usage.input).toBe(100);
    expect(views[1].status).toBe("skipped");
  });

  it("seeds unfinished steps from the topology as pending", () => {
    const detail: RunDetail = {
      id: "r2",
      workflow_name: "demo",
      title: null,
      description: "A sample description",
      status: "running",
      project: "ai-harness",
      node_count: 3,
      recorded_at: NOW,
      started_at: null,
      ended_at: null,
      ab_pair_id: null,
      ab_arm: null,
      ab_label: null,
      graph: [
        { id: "explore", depends_on: [] },
        { id: "plan", depends_on: ["explore"] },
        { id: "implement", depends_on: ["plan"] },
      ],
      // Only the first step has finished so far.
      nodes: [
        {
          node_id: "explore",
          ordinal: 0,
          status: "success",
          provider: "claude",
          model: "sonnet",
          output: "explored",
          iterations: 1,
          converged: null,
          note: null,
          input_tokens: 10,
          output_tokens: 5,
          cache_read: null,
          cache_write: null,
          started_at: NOW,
          ended_at: NOW,
          artifact_content: null,
        },
      ],
    };
    const views = nodesFromDetail(detail);
    // The whole DAG shows even though only one node has a persisted row.
    expect(views.map((v) => v.id)).toEqual(["explore", "plan", "implement"]);
    expect(views[0].status).toBe("success");
    expect(views[1].status).toBe("pending");
    expect(views[2].status).toBe("pending");
    expect(views[2].depends_on).toEqual(["plan"]);
  });

  it("carries artifact from topology and artifact_content from persisted row", () => {
    const detail: RunDetail = {
      id: "r3",
      workflow_name: "demo",
      title: null,
      description: null,
      status: "completed",
      project: null,
      node_count: 1,
      recorded_at: NOW,
      started_at: null,
      ended_at: null,
      ab_pair_id: null,
      ab_arm: null,
      ab_label: null,
      graph: [{ id: "explore", depends_on: [], artifact: "exploration.md" }],
      nodes: [
        {
          node_id: "explore",
          ordinal: 0,
          status: "success",
          provider: "claude",
          model: "sonnet",
          output: "done",
          iterations: 1,
          converged: null,
          note: null,
          input_tokens: 10,
          output_tokens: 5,
          cache_read: null,
          cache_write: null,
          started_at: null,
          ended_at: null,
          artifact_content: "# Explore\nsample",
        },
      ],
    };
    const views = nodesFromDetail(detail);
    expect(views[0].artifact).toBe("exploration.md");
    expect(views[0].artifact_content).toBe("# Explore\nsample");
  });

  it("falls back to null artifact fields when absent", () => {
    const detail: RunDetail = {
      id: "r4",
      workflow_name: "demo",
      title: null,
      description: null,
      status: "completed",
      project: null,
      node_count: 1,
      recorded_at: NOW,
      started_at: null,
      ended_at: null,
      ab_pair_id: null,
      ab_arm: null,
      ab_label: null,
      graph: [{ id: "build", depends_on: [] }],
      nodes: [
        {
          node_id: "build",
          ordinal: 0,
          status: "success",
          provider: null,
          model: null,
          output: "",
          iterations: 1,
          converged: null,
          note: null,
          input_tokens: null,
          output_tokens: null,
          cache_read: null,
          cache_write: null,
          started_at: null,
          ended_at: null,
          artifact_content: null,
        },
      ],
    };
    const views = nodesFromDetail(detail);
    expect(views[0].artifact).toBeNull();
    expect(views[0].artifact_content).toBeNull();
  });
});
