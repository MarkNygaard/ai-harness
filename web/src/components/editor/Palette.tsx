import { Bot, FileCode2, GripVertical, Repeat, SquareTerminal, Terminal } from "lucide-react";
import type { NodeKindId, NodeKindInfo } from "@/types/authoring";

const ICON: Record<string, typeof Bot> = {
  prompt: Bot,
  command: FileCode2,
  bash: SquareTerminal,
  loop: Repeat,
  script: Terminal,
};

/** Left palette: the building blocks (catalog node kinds). Click or drag to add. */
export function Palette({
  kinds,
  onAdd,
}: {
  kinds: NodeKindInfo[];
  onAdd: (kind: NodeKindId) => void;
}) {
  return (
    <div className="flex w-60 flex-none flex-col border-r border-border bg-card">
      <div className="border-b border-border px-4 py-3 text-sm font-semibold">Building blocks</div>
      <div className="flex flex-col gap-1.5 overflow-auto p-2">
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
      </div>
      <div className="mt-auto border-t border-border p-3 text-[11px] leading-snug text-muted-foreground">
        Click or drag a block onto the canvas. Connect nodes to set dependencies.
      </div>
    </div>
  );
}
