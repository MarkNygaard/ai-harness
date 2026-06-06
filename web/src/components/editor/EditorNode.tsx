import { Handle, Position, type NodeProps } from "@xyflow/react";
import {
  Bot,
  CircleSlash,
  FileCode2,
  Hand,
  Repeat,
  Settings2,
  SquareTerminal,
  Terminal,
  Trash2,
} from "lucide-react";
import { cn } from "@/lib/utils";
import type { NodeKindId } from "@/types/authoring";
import { type EditorNodeData, nodeKind } from "./graph";
import { useEditorActions } from "./context";

const KIND_ICON: Record<NodeKindId, typeof Bot> = {
  prompt: Bot,
  command: FileCode2,
  bash: SquareTerminal,
  loop: Repeat,
  script: Terminal,
  approval: Hand,
  cancel: CircleSlash,
};

const KIND_LABEL: Record<NodeKindId, string> = {
  prompt: "Agent step",
  command: "Command",
  bash: "Shell",
  loop: "Loop",
  script: "Script",
  approval: "Approval",
  cancel: "Cancel",
};

/** A draggable, connectable workflow step on the editor canvas. */
export function EditorNode({ id, data }: NodeProps) {
  const { node } = data as EditorNodeData;
  const kind = nodeKind(node);
  const Icon = KIND_ICON[kind];
  const { onConfigure, onDelete, selectedId } = useEditorActions();
  const selected = selectedId === id;

  const providerModel =
    node.model ??
    node.provider ??
    (kind === "loop" ? node.loop?.model : undefined);

  return (
    <div
      className={cn(
        "w-[200px] rounded-lg border bg-card shadow-sm transition-colors",
        selected ? "border-accent-orange" : "border-border",
      )}
    >
      <Handle
        type="target"
        position={Position.Top}
        className="!h-2 !w-2 !border-0 !bg-border"
      />
      <div className="flex items-center gap-2 px-3 py-2">
        <Icon className="h-4 w-4 shrink-0 text-accent-orange" />
        <div className="min-w-0 flex-1">
          <div
            className="truncate text-[13px] font-semibold text-card-foreground"
            title={node.id}
          >
            {node.id || "(unnamed)"}
          </div>
          <div className="truncate text-[10px] text-muted-foreground">
            {KIND_LABEL[kind]}
            {providerModel ? ` · ${providerModel}` : ""}
          </div>
        </div>
        <button
          type="button"
          title="Configure"
          onClick={(e) => {
            e.stopPropagation();
            onConfigure(id);
          }}
          className="rounded p-1 text-muted-foreground hover:bg-secondary hover:text-foreground"
        >
          <Settings2 className="h-3.5 w-3.5" />
        </button>
        <button
          type="button"
          title="Delete"
          onClick={(e) => {
            e.stopPropagation();
            onDelete(id);
          }}
          className="rounded p-1 text-muted-foreground hover:bg-destructive/15 hover:text-destructive"
        >
          <Trash2 className="h-3.5 w-3.5" />
        </button>
      </div>
      <Handle
        type="source"
        position={Position.Bottom}
        className="!h-2 !w-2 !border-0 !bg-border"
      />
    </div>
  );
}
