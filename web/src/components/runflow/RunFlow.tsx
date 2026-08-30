import { useEffect, useMemo, useRef, useState } from "react";
import {
  Background,
  BackgroundVariant,
  Controls,
  ReactFlow,
  useReactFlow,
  type NodeTypes,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import type { NodeView } from "@/types/run";
import { useTheme } from "@/lib/theme";
import { layoutRun } from "./layout";
import { RunNode } from "./RunNode";
import { StepDialog } from "./StepDialog";

const nodeTypes: NodeTypes = { run: RunNode };

const FIT = { padding: 0.15 };

/**
 * Keep the whole graph in view as steps appear.
 *
 * React Flow's `fitView` prop fits **once**, on first render. A run that is
 * still going gains nodes as it goes, so the view stays fitted to the two or
 * three steps that existed at mount and everything after lands off-screen —
 * which reads as the page opening zoomed in, and only on live runs, which is
 * why it seemed intermittent.
 *
 * Refitting stops the moment the viewer pans or zooms. Someone who has moved
 * the view is looking at something, and yanking it back every time a step
 * finishes would be worse than the bug. `onMoveStart` passes `null` for
 * programmatic moves, so our own fits do not count as interaction.
 */
function FitToSteps({
  signature,
  moved,
}: {
  signature: string;
  moved: boolean;
}) {
  const { fitView } = useReactFlow();
  useEffect(() => {
    if (moved) return;
    // After paint: nodes have to be measured before they can be fitted, and on
    // the first render of a freshly-mounted graph they are not yet.
    const frame = requestAnimationFrame(() => {
      void fitView(FIT);
    });
    return () => cancelAnimationFrame(frame);
  }, [signature, moved, fitView]);
  return null;
}

/**
 * Renders the actual executed workflow DAG: one node per step (with live status,
 * elapsed time, provider/model, tokens, hover details) and dependency edges that
 * animate while a downstream step is running.
 */
export function RunFlow({ nodes: views }: { nodes: NodeView[] }) {
  // React Flow paints its own controls and edges, so it has to be told
  // which theme is active rather than inheriting it from the page.
  const { resolved } = useTheme();
  const { nodes, edges } = useMemo(() => layoutRun(views), [views]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [moved, setMoved] = useState(false);
  // Which steps are on screen. Ids rather than a count, so a step being
  // replaced rather than added still refits.
  const signature = useMemo(() => nodes.map((n) => n.id).join("|"), [nodes]);
  const movedRef = useRef(moved);
  movedRef.current = moved;

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
        fitViewOptions={FIT}
        onMoveStart={(event) => {
          // `null` is one of our own `fitView` calls; only a real gesture
          // means the viewer has taken over.
          if (event && !movedRef.current) setMoved(true);
        }}
        minZoom={0.2}
        maxZoom={1.5}
        nodesConnectable={false}
        onNodeClick={(_e, n) => setSelectedId(n.id)}
        colorMode={resolved}
        proOptions={{ hideAttribution: true }}
        className="bg-transparent"
      >
        <Background
          variant={BackgroundVariant.Dots}
          gap={20}
          size={1}
          color="var(--border)"
        />
        <Controls showInteractive={false} className="border-border! bg-card!" />
        <FitToSteps signature={signature} moved={moved} />
      </ReactFlow>
      <StepDialog view={selected} onClose={() => setSelectedId(null)} />
    </>
  );
}
