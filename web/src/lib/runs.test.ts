import { describe, expect, it } from "vitest";
import { liveReducer, nodesFromDetail } from "./runs";
import type { RunDetail, RunEvent } from "@/types/run";

const NOW = "2026-01-01T00:00:10.000Z";

function reduce(events: RunEvent[]) {
  let state = liveReducer(undefined as never, { type: "reset" });
  for (const event of events) state = liveReducer(state, { type: "event", event, now: NOW });
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
  });

  it("marks a node running then merges its finished record", () => {
    const state = reduce([
      { type: "run_started", workflow: "demo", total_nodes: 1, nodes: [{ id: "a", depends_on: [] }] },
      { type: "node_started", node_id: "a", provider: "claude", model: "sonnet" },
      {
        type: "node_finished",
        node: {
          id: "a",
          status: "success",
          provider: "claude",
          model: "sonnet",
          output: "ok",
          usage: { input: 100, output: 20, cache_read: null, cache_write: null },
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

  it("records terminal run status", () => {
    const state = reduce([
      { type: "run_started", workflow: "d", total_nodes: 0, nodes: [] },
      { type: "run_finished", status: "completed" },
    ]);
    expect(state.status).toBe("completed");
  });

  it("stamps started_at on node_started when the event has no timestamp", () => {
    const state = reduce([
      { type: "run_started", workflow: "d", total_nodes: 1, nodes: [{ id: "a", depends_on: [] }] },
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
      status: "completed",
      project: null,
      node_count: 2,
      recorded_at: NOW,
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
      status: "running",
      project: "ai-harness",
      node_count: 3,
      recorded_at: NOW,
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
});
