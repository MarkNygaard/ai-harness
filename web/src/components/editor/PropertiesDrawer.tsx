import { X } from "lucide-react";
import type { Catalog, EditorNode, NodeKindId } from "@/types/authoring";
import { emptyNode, nodeKind } from "@/lib/workflow-yaml";
import { useCategories } from "@/lib/categories";

/** Body fields that define a node's kind — cleared/swapped when the kind changes. */
const BODY_KEYS = [
  "prompt",
  "bash",
  "command",
  "script",
  "runtime",
  "deps",
  "loop",
  "approval",
  "cancel",
] as const satisfies readonly (keyof EditorNode)[];

/** Right drawer: edit the selected node's id, kind, body, and AI options. */
export function PropertiesDrawer({
  node,
  catalog,
  onChange,
  onClose,
}: {
  node: EditorNode;
  catalog: Catalog | undefined;
  onChange: (next: EditorNode) => void;
  onClose: () => void;
}) {
  const kind = nodeKind(node);
  const categories = useCategories();
  const set = (patch: Partial<EditorNode>) => onChange({ ...node, ...patch });

  // Switching kind swaps the body but keeps id/edges and all other options
  // (when, category, output_format, provider/model/context/trigger_rule/…).
  const changeKind = (next: NodeKindId) => {
    const fresh = emptyNode(next, node.id);
    const cleared: Partial<EditorNode> = { ...node };
    const body: Record<string, unknown> = {};
    for (const k of BODY_KEYS) {
      delete cleared[k];
      if (fresh[k] !== undefined) body[k] = fresh[k];
    }
    onChange({ ...cleared, ...body } as EditorNode);
  };

  const provider = node.provider ?? "";
  const providerModels =
    catalog?.providers.find((p) => p.id === provider)?.models ?? [];

  return (
    <div className="flex w-80 flex-none flex-col border-l border-border bg-card">
      <div className="flex items-center justify-between border-b border-border px-4 py-3">
        <span className="text-sm font-semibold">Step settings</span>
        <button
          type="button"
          onClick={onClose}
          className="rounded p-1 hover:bg-secondary"
        >
          <X className="h-4 w-4" />
        </button>
      </div>

      <div className="flex flex-col gap-3 overflow-auto p-4 text-[13px]">
        <Field label="Step id">
          <input
            className={inputCls}
            value={node.id}
            onChange={(e) => set({ id: e.target.value })}
          />
        </Field>

        <Field label="Type">
          <select
            className={inputCls}
            value={kind}
            onChange={(e) => changeKind(e.target.value as NodeKindId)}
          >
            {(catalog?.node_kinds ?? []).map((k) => (
              <option key={k.kind} value={k.kind}>
                {k.label}
              </option>
            ))}
          </select>
        </Field>

        {/* Body by kind */}
        {kind === "prompt" && (
          <Field label="Prompt">
            <textarea
              className={textareaCls}
              rows={6}
              value={node.prompt ?? ""}
              onChange={(e) => set({ prompt: e.target.value })}
            />
          </Field>
        )}
        {kind === "bash" && (
          <Field label="Bash">
            <textarea
              className={`${textareaCls} font-mono`}
              rows={6}
              value={node.bash ?? ""}
              onChange={(e) => set({ bash: e.target.value })}
            />
          </Field>
        )}
        {kind === "command" && (
          <Field label="Command">
            <input
              className={inputCls}
              list="harness-commands"
              value={node.command ?? ""}
              onChange={(e) => set({ command: e.target.value })}
              placeholder="implement-tasks"
            />
            <datalist id="harness-commands">
              {(catalog?.commands ?? []).map((c) => (
                <option key={c.name} value={c.name} />
              ))}
            </datalist>
          </Field>
        )}
        {kind === "script" && (
          <>
            <Field label="Runtime">
              <select
                className={inputCls}
                value={node.runtime ?? "bun"}
                onChange={(e) =>
                  set({ runtime: e.target.value as EditorNode["runtime"] })
                }
              >
                <option value="bun">bun (TS/JS)</option>
                <option value="uv">uv (Python)</option>
              </select>
            </Field>
            <Field label="Script">
              <textarea
                className={`${textareaCls} font-mono`}
                rows={6}
                value={node.script ?? ""}
                onChange={(e) => set({ script: e.target.value })}
              />
            </Field>
          </>
        )}
        {kind === "loop" && (
          <>
            <Field label="Loop prompt">
              <textarea
                className={textareaCls}
                rows={5}
                value={node.loop?.prompt ?? ""}
                onChange={(e) =>
                  set({ loop: { ...loopOf(node), prompt: e.target.value } })
                }
              />
            </Field>
            <div className="grid grid-cols-2 gap-2">
              <Field label="Until signal">
                <input
                  className={inputCls}
                  value={node.loop?.until ?? ""}
                  onChange={(e) =>
                    set({ loop: { ...loopOf(node), until: e.target.value } })
                  }
                />
              </Field>
              <Field label="Max iterations">
                <input
                  type="number"
                  min={1}
                  className={inputCls}
                  value={node.loop?.max_iterations ?? 3}
                  onChange={(e) =>
                    set({
                      loop: {
                        ...loopOf(node),
                        max_iterations: Number(e.target.value),
                      },
                    })
                  }
                />
              </Field>
            </div>
          </>
        )}
        {kind === "approval" && (
          <>
            <Field label="Approval message">
              <textarea
                className={textareaCls}
                rows={4}
                value={node.approval?.message ?? ""}
                onChange={(e) =>
                  set({
                    approval: {
                      ...(node.approval ?? { message: "" }),
                      message: e.target.value,
                    },
                  })
                }
              />
            </Field>
            <label className="flex items-center gap-2 text-[12px] text-muted-foreground">
              <input
                type="checkbox"
                checked={node.approval?.capture_response ?? false}
                onChange={(e) =>
                  set({
                    approval: {
                      ...(node.approval ?? { message: "" }),
                      capture_response: e.target.checked,
                    },
                  })
                }
              />
              Capture the approver’s response
            </label>
          </>
        )}
        {kind === "cancel" && (
          <Field label="Cancel reason">
            <textarea
              className={textareaCls}
              rows={3}
              value={node.cancel ?? ""}
              onChange={(e) => set({ cancel: e.target.value })}
              placeholder="Refusing to proceed: …"
            />
          </Field>
        )}

        <Field label="When (condition)">
          <input
            className={`${inputCls} font-mono`}
            value={node.when ?? ""}
            onChange={(e) => set({ when: e.target.value || undefined })}
            placeholder="$classify.output.type == 'BUG'"
          />
        </Field>

        <div className="mt-1 border-t border-border pt-3 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
          AI options
        </div>

        <Field label="Provider">
          <select
            className={inputCls}
            value={provider}
            onChange={(e) => set({ provider: e.target.value || undefined })}
          >
            <option value="">(workflow default)</option>
            {(catalog?.providers ?? []).map((p) => (
              <option key={p.id} value={p.id}>
                {p.label}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Model">
          <input
            className={inputCls}
            list="harness-models"
            value={node.model ?? ""}
            onChange={(e) => set({ model: e.target.value || undefined })}
            placeholder="(default)"
          />
          <datalist id="harness-models">
            {providerModels.map((m) => (
              <option key={m} value={m} />
            ))}
          </datalist>
        </Field>
        <div className="grid grid-cols-2 gap-2">
          <Field label="Context">
            <select
              className={inputCls}
              value={node.context ?? "shared"}
              onChange={(e) =>
                set({ context: e.target.value as EditorNode["context"] })
              }
            >
              {(catalog?.context_modes ?? ["fresh", "shared"]).map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Trigger rule">
            <select
              className={inputCls}
              value={node.trigger_rule ?? "all_success"}
              onChange={(e) =>
                set({
                  trigger_rule: e.target.value as EditorNode["trigger_rule"],
                })
              }
            >
              {(catalog?.trigger_rules ?? ["all_success"]).map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </select>
          </Field>
        </div>
        <Field label="Category">
          <select
            className={inputCls}
            value={node.category ?? ""}
            onChange={(e) => set({ category: e.target.value || undefined })}
          >
            <option value="">(none — status colour)</option>
            {(categories.data ?? []).map((c) => (
              <option key={c.id} value={c.id}>
                {c.label}
              </option>
            ))}
          </select>
        </Field>
        {(kind === "bash" || kind === "script") && (
          <Field label="Timeout (ms)">
            <input
              type="number"
              min={0}
              className={inputCls}
              value={node.timeout ?? ""}
              onChange={(e) =>
                set({
                  timeout: e.target.value ? Number(e.target.value) : undefined,
                })
              }
            />
          </Field>
        )}
      </div>
    </div>
  );
}

function loopOf(node: EditorNode) {
  return node.loop ?? { prompt: "", until: "DONE", max_iterations: 3 };
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[11px] font-medium text-muted-foreground">
        {label}
      </span>
      {children}
    </label>
  );
}

const inputCls =
  "h-8 rounded-md border border-input bg-transparent px-2 text-[13px] outline-none focus:ring-2 focus:ring-ring";
const textareaCls =
  "rounded-md border border-input bg-transparent p-2 text-[12px] outline-none focus:ring-2 focus:ring-ring resize-y";
