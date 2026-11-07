import { Handle, Position, type NodeProps } from "@xyflow/react";
import {
  Bot,
  CircleSlash,
  FileCode2,
  FileText,
  Hand,
  Repeat,
  Settings2,
  Split,
  SquareTerminal,
  Terminal,
  Trash2,
  Zap,
} from "lucide-react";
import { cn } from "@/lib/utils";
import type { NodeKindId } from "@/types/authoring";
import { type EditorNodeData, nodeKind } from "./graph";
import { useEditorActions } from "./context";
import { useLinearConnections } from "@/lib/linear";

const KIND_ICON: Record<NodeKindId, typeof Bot> = {
  prompt: Bot,
  command: FileCode2,
  bash: SquareTerminal,
  loop: Repeat,
  script: Terminal,
  approval: Hand,
  cancel: CircleSlash,
  linear: Zap,
  route: Split,
};

const KIND_LABEL: Record<NodeKindId, string> = {
  prompt: "Agent step",
  command: "Command",
  bash: "Shell",
  loop: "Loop",
  script: "Script",
  approval: "Approval",
  cancel: "Cancel",
  linear: "Linear",
  route: "Route",
};

/** A draggable, connectable workflow step on the editor canvas. */
export function EditorNode({ id, data }: NodeProps) {
  const { node } = data as EditorNodeData;
  const kind = nodeKind(node);
  const Icon = KIND_ICON[kind];
  const { onConfigure, onDelete, selectedId } = useEditorActions();
  const selected = selectedId === id;
  // A linear step with no account connected anywhere still saves and still
  // runs -- it just does nothing and lets the run continue. Saying so on the
  // node is the whole point: the step is present but inert, which is not
  // something the graph would otherwise show.
  const linear = useLinearConnections();
  const inert =
    kind === "linear" && linear.isSuccess && (linear.data?.length ?? 0) === 0;

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
        <Icon
          className={cn(
            "h-4 w-4 shrink-0",
            inert ? "text-muted-foreground" : "text-accent-orange",
          )}
        />
        <div className="min-w-0 flex-1">
          <div
            className="truncate text-[13px] font-semibold text-card-foreground"
            title={node.id}
          >
            {node.id || "(unnamed)"}
          </div>
          <div
            className="truncate text-[10px] text-muted-foreground"
            title={
              inert
                ? "No Linear account is connected, so this step does nothing and the run carries on."
                : undefined
            }
          >
            {KIND_LABEL[kind]}
            {inert ? " · needs Linear" : ""}
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
      {node.artifact && (
        <div
          className="flex items-center gap-1.5 border-t border-border px-3 py-1.5 text-[10px] text-muted-foreground"
          title={`Produces ${node.artifact}`}
        >
          <FileText className="h-3 w-3 shrink-0" />
          <span className="truncate font-mono">{node.artifact}</span>
        </div>
      )}
      {node.route ? (
        <RouteExits
          routes={node.route.routes.map((r) => r.name)}
          fallback={node.route.default}
        />
      ) : (
        <Handle
          type="source"
          position={Position.Bottom}
          className="!h-2 !w-2 !border-0 !bg-border"
        />
      )}
    </div>
  );
}

/**
 * One exit per route, each its own source handle (id = the route's name), so
 * connecting from an exit says which route the next step belongs to.
 */
function RouteExits({
  routes,
  fallback,
}: {
  routes: string[];
  fallback?: string;
}) {
  return (
    <div className="flex border-t border-border">
      {routes.map((name, i) => (
        <div
          key={`${i}-${name}`}
          className="relative min-w-0 flex-1 px-1 pt-1 pb-2 text-center"
          title={name === fallback ? `${name} (default)` : name}
        >
          <span
            className={cn(
              "block truncate font-mono text-[10px]",
              name === fallback
                ? "text-card-foreground"
                : "text-muted-foreground",
            )}
          >
            {name || "?"}
          </span>
          <Handle
            type="source"
            id={name}
            position={Position.Bottom}
            className="!h-2 !w-2 !border-0 !bg-accent-orange"
          />
        </div>
      ))}
    </div>
  );
}
