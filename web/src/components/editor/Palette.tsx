import { Bot, FileCode2, GripVertical, Repeat, SquareTerminal, Terminal } from "lucide-react";
import type { NodeKindId, NodeKindInfo, PrebuiltStep } from "@/types/authoring";
import { nodeKind } from "@/lib/workflow-yaml";

const ICON: Record<string, typeof Bot> = {
  prompt: Bot,
  command: FileCode2,
  bash: SquareTerminal,
  loop: Repeat,
  script: Terminal,
};

/** Left palette: raw building blocks + curated prebuilt steps. Click or drag to add. */
export function Palette({
  kinds,
  prebuilt,
  onAdd,
  onAddPrebuilt,
}: {
  kinds: NodeKindInfo[];
  prebuilt: PrebuiltStep[];
  onAdd: (kind: NodeKindId) => void;
  onAddPrebuilt: (step: PrebuiltStep) => void;
}) {
  return (
    <div className="flex w-60 flex-none flex-col border-r border-border bg-card">
      <div className="flex flex-col gap-1.5 overflow-auto p-2">
        <div className="px-2 py-1 text-sm font-semibold">Building blocks</div>
        {kinds.map((k) => {
          const Icon = ICON[k.kind] ?? Bot;
          return (
            <button
              key={k.kind}
              type="button"
              draggable
              onDragStart={(e) => e.dataTransfer.setData("application/harness-node-kind", k.kind)}
              onClick={() => onAdd(k.kind as NodeKindId)}
              title={k.description}
              className="group flex items-center gap-2.5 rounded-md border border-border bg-background px-2.5 py-2 text-left hover:border-accent-orange/50"
            >
              <Icon className="h-4 w-4 shrink-0 text-accent-orange" />
              <span className="flex-1 text-[13px] font-medium">{k.label}</span>
              <GripVertical className="h-3.5 w-3.5 text-muted-foreground opacity-0 group-hover:opacity-100" />
            </button>
          );
        })}

        <div className="mt-3 px-2 py-1 text-sm font-semibold">Prebuilt steps</div>
        {prebuilt.map((s) => {
          const Icon = ICON[nodeKind(s.node)] ?? Bot;
          return (
            <button
              key={s.id}
              type="button"
              draggable
              onDragStart={(e) =>
                e.dataTransfer.setData("application/harness-prebuilt-step", s.id)
              }
              onClick={() => onAddPrebuilt(s)}
              title={s.description}
              className="group flex items-center gap-2.5 rounded-md border border-border bg-background px-2.5 py-2 text-left hover:border-accent-orange/50"
            >
              <Icon className="h-4 w-4 shrink-0 text-accent-orange" />
              <span className="flex-1 text-[13px] font-medium">{s.label}</span>
              <GripVertical className="h-3.5 w-3.5 text-muted-foreground opacity-0 group-hover:opacity-100" />
            </button>
          );
        })}
      </div>
      <div className="mt-auto border-t border-border p-3 text-[11px] leading-snug text-muted-foreground">
        Click or drag a block onto the canvas. Connect nodes to set dependencies.
      </div>
    </div>
  );
}
