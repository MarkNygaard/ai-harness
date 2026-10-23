import {
  Bot,
  FileCode2,
  GripVertical,
  Repeat,
  SquareTerminal,
  Terminal,
  Trash2,
} from "lucide-react";
import type {
  NodeKindId,
  NodeKindInfo,
  PrebuiltStep,
  SavedStep,
} from "@/types/authoring";
import { nodeKind } from "@/lib/workflow-yaml";
import { useDeleteStep } from "@/lib/authoring";

const ICON: Record<string, typeof Bot> = {
  prompt: Bot,
  command: FileCode2,
  bash: SquareTerminal,
  loop: Repeat,
  script: Terminal,
};

/** Drag payload types, read back by the canvas's drop handler. */
export const DRAG_PREBUILT = "application/harness-prebuilt-step";
export const DRAG_SAVED = "application/harness-saved-step";

const ROW =
  "group flex items-center gap-2.5 rounded-md border border-border bg-background px-2.5 py-2 text-left hover:border-accent-orange/50";

/** Left palette: raw building blocks, curated prebuilt steps, and the steps
 *  people saved. Click or drag to add. */
export function Palette({
  kinds,
  prebuilt,
  saved,
  onAdd,
  onAddPrebuilt,
}: {
  kinds: NodeKindInfo[];
  prebuilt: PrebuiltStep[];
  saved: SavedStep[];
  onAdd: (kind: NodeKindId) => void;
  onAddPrebuilt: (step: PrebuiltStep) => void;
}) {
  const remove = useDeleteStep();
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
              onDragStart={(e) =>
                e.dataTransfer.setData("application/harness-node-kind", k.kind)
              }
              onClick={() => onAdd(k.kind as NodeKindId)}
              title={k.description}
              className={ROW}
            >
              <Icon className="h-4 w-4 shrink-0 text-accent-orange" />
              <span className="flex-1 text-[13px] font-medium">{k.label}</span>
              <GripVertical className="h-3.5 w-3.5 text-muted-foreground opacity-0 group-hover:opacity-100" />
            </button>
          );
        })}

        <div className="mt-3 px-2 py-1 text-sm font-semibold">
          Prebuilt steps
        </div>
        {prebuilt.map((s) => {
          const Icon = ICON[nodeKind(s.node)] ?? Bot;
          return (
            <button
              key={s.id}
              type="button"
              draggable
              onDragStart={(e) => e.dataTransfer.setData(DRAG_PREBUILT, s.id)}
              onClick={() => onAddPrebuilt(s)}
              title={s.description}
              className={ROW}
            >
              <Icon className="h-4 w-4 shrink-0 text-accent-orange" />
              <span className="flex-1 text-[13px] font-medium">{s.label}</span>
              <GripVertical className="h-3.5 w-3.5 text-muted-foreground opacity-0 group-hover:opacity-100" />
            </button>
          );
        })}

        <div className="mt-3 px-2 py-1 text-sm font-semibold">Saved steps</div>
        {saved.length === 0 && (
          <p className="px-2 text-[11px] leading-snug text-muted-foreground">
            None yet. Select a step and choose Save as step to reuse it.
          </p>
        )}
        {saved.map((s) => {
          const Icon = ICON[nodeKind(s.node)] ?? Bot;
          const by = s.created_by_label
            ? `\nSaved by ${s.created_by_label}`
            : "";
          return (
            <div key={s.id} className={ROW}>
              <button
                type="button"
                draggable
                onDragStart={(e) => e.dataTransfer.setData(DRAG_SAVED, s.id)}
                onClick={() => onAddPrebuilt(s)}
                title={`${s.description}${by}`.trim()}
                className="flex min-w-0 flex-1 items-center gap-2.5 text-left"
              >
                <Icon className="h-4 w-4 shrink-0 text-accent-orange" />
                <span className="flex-1 truncate text-[13px] font-medium">
                  {s.label}
                </span>
              </button>
              {s.deletable && (
                <button
                  type="button"
                  aria-label={`Delete ${s.label}`}
                  title="Delete this saved step. Workflows that use it keep their copy."
                  disabled={remove.isPending}
                  onClick={() => {
                    if (window.confirm(`Delete the saved step “${s.label}”?`)) {
                      remove.mutate(s.id);
                    }
                  }}
                  className="rounded p-0.5 text-muted-foreground opacity-0 hover:text-destructive group-hover:opacity-100"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </button>
              )}
            </div>
          );
        })}
        {remove.isError && (
          <p className="px-2 text-[11px] text-destructive">
            {remove.error.message}
          </p>
        )}
      </div>
      <div className="mt-auto border-t border-border p-3 text-[11px] leading-snug text-muted-foreground">
        Click or drag a block onto the canvas. Connect nodes to set
        dependencies.
      </div>
    </div>
  );
}
