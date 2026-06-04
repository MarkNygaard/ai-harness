import { useMemo, useState } from "react";
import {
  Background,
  BackgroundVariant,
  Controls,
  ReactFlow,
  type NodeTypes,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import type { NodeView } from "@/types/run";
import { layoutRun } from "./layout";
import { RunNode } from "./RunNode";
import { StepDialog } from "./StepDialog";

const nodeTypes: NodeTypes = { run: RunNode };

/**
 * Renders the actual executed workflow DAG: one node per step (with live status,
 * elapsed time, provider/model, tokens, hover details) and dependency edges that
 * animate while a downstream step is running.
 */
export function RunFlow({ nodes: views }: { nodes: NodeView[] }) {
  const { nodes, edges } = useMemo(() => layoutRun(views), [views]);
  const [selectedId, setSelectedId] = useState<string | null>(null);

  if (views.length === 0) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        No steps to display.
      </div>
    );
  }

  const selected = views.find((v) => v.id === selectedId) ?? null;

  return (
    <>
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        fitView
        fitViewOptions={{ padding: 0.15 }}
        minZoom={0.2}
        maxZoom={1.5}
        nodesConnectable={false}
        onNodeClick={(_e, n) => setSelectedId(n.id)}
        proOptions={{ hideAttribution: true }}
        className="bg-transparent"
      >
        <Background variant={BackgroundVariant.Dots} gap={20} size={1} color="var(--border)" />
        <Controls showInteractive={false} className="!border-border !bg-card" />
      </ReactFlow>
      <StepDialog view={selected} onClose={() => setSelectedId(null)} />
    </>
  );
}
