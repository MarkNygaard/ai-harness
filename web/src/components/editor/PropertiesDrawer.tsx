import { X } from "lucide-react";
import type { Catalog, EditorNode, NodeKindId } from "@/types/authoring";
import { emptyNode, nodeKind } from "@/lib/workflow-yaml";
import { useCategories } from "@/lib/categories";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";

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
          <Input
            value={node.id}
            onChange={(e) => set({ id: e.target.value })}
          />
        </Field>

        <SelectField
          label="Type"
          value={kind}
          onValueChange={(v) => changeKind(v as NodeKindId)}
        >
          {(catalog?.node_kinds ?? []).map((k) => (
            <SelectItem key={k.kind} value={k.kind}>
              {k.label}
            </SelectItem>
          ))}
        </SelectField>

        {/* Body by kind */}
        {kind === "prompt" && (
          <Field label="Prompt">
            <Textarea
              rows={6}
              value={node.prompt ?? ""}
              onChange={(e) => set({ prompt: e.target.value })}
            />
          </Field>
        )}
        {kind === "bash" && (
          <Field label="Bash">
            <Textarea
              className="font-mono"
              rows={6}
              value={node.bash ?? ""}
              onChange={(e) => set({ bash: e.target.value })}
            />
          </Field>
        )}
        {kind === "command" && (
          <Field label="Command">
            <Input
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
            <SelectField
              label="Runtime"
              value={node.runtime ?? "bun"}
              onValueChange={(v) =>
                set({ runtime: v as EditorNode["runtime"] })
              }
            >
              <SelectItem value="bun">bun (TS/JS)</SelectItem>
              <SelectItem value="uv">uv (Python)</SelectItem>
            </SelectField>
            <Field label="Script">
              <Textarea
                className="font-mono"
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
              <Textarea
                rows={5}
                value={node.loop?.prompt ?? ""}
                onChange={(e) =>
                  set({ loop: { ...loopOf(node), prompt: e.target.value } })
                }
              />
            </Field>
            <div className="grid grid-cols-2 gap-2">
              <Field label="Until signal">
                <Input
                  value={node.loop?.until ?? ""}
                  onChange={(e) =>
                    set({ loop: { ...loopOf(node), until: e.target.value } })
                  }
                />
              </Field>
              <Field label="Max iterations">
                <Input
                  type="number"
                  min={1}
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
              <Textarea
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
            <Textarea
              rows={3}
              value={node.cancel ?? ""}
              onChange={(e) => set({ cancel: e.target.value })}
              placeholder="Refusing to proceed: …"
            />
          </Field>
        )}

        <Field label="When (condition)">
          <Input
            className="font-mono"
            value={node.when ?? ""}
            onChange={(e) => set({ when: e.target.value || undefined })}
            placeholder="$classify.output.type == 'BUG'"
          />
        </Field>

        <div className="mt-1 border-t border-border pt-3 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
          AI options
        </div>

        <SelectField
          label="CLI"
          value={provider || DEFAULT_SENTINEL}
          onValueChange={(v) =>
            set({ provider: v === DEFAULT_SENTINEL ? undefined : v })
          }
        >
          <SelectItem value={DEFAULT_SENTINEL}>(workflow default)</SelectItem>
          {(catalog?.providers ?? []).map((p) => (
            <SelectItem key={p.id} value={p.id}>
              {p.label}
            </SelectItem>
          ))}
        </SelectField>
        <Field label="Model">
          <Input
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
          <SelectField
            label="Context"
            value={node.context ?? "shared"}
            onValueChange={(v) => set({ context: v as EditorNode["context"] })}
          >
            {(catalog?.context_modes ?? ["fresh", "shared"]).map((c) => (
              <SelectItem key={c} value={c}>
                {c}
              </SelectItem>
            ))}
          </SelectField>
          <SelectField
            label="Trigger rule"
            value={node.trigger_rule ?? "all_success"}
            onValueChange={(v) =>
              set({ trigger_rule: v as EditorNode["trigger_rule"] })
            }
          >
            {(catalog?.trigger_rules ?? ["all_success"]).map((t) => (
              <SelectItem key={t} value={t}>
                {t}
              </SelectItem>
            ))}
          </SelectField>
        </div>
        <SelectField
          label="Category"
          value={node.category ?? NONE_SENTINEL}
          onValueChange={(v) =>
            set({ category: v === NONE_SENTINEL ? undefined : v })
          }
        >
          <SelectItem value={NONE_SENTINEL}>(none — status colour)</SelectItem>
          {(categories.data ?? []).map((c) => (
            <SelectItem key={c.id} value={c.id}>
              {c.label}
            </SelectItem>
          ))}
        </SelectField>
        <Field label="Artifact">
          <Input
            type="text"
            placeholder="e.g. exploration.md"
            value={node.artifact ?? ""}
            onChange={(e) => set({ artifact: e.target.value || undefined })}
          />
        </Field>
        {(kind === "bash" || kind === "script") && (
          <Field label="Timeout (ms)">
            <Input
              type="number"
              min={0}
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

/** Sentinels for nullable selects — Base UI Select needs a concrete item value. */
const DEFAULT_SENTINEL = "__default__";
const NONE_SENTINEL = "__none__";

/** A labelled shadcn Select (replaces the native `<select>` for a styled popup).
 *  Not wrapped in a `<label>` — the trigger is a button. */
function SelectField({
  label,
  value,
  onValueChange,
  children,
}: {
  label: string;
  value: string;
  onValueChange: (v: string) => void;
  children: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1">
      <span className="text-[11px] font-medium text-muted-foreground">
        {label}
      </span>
      <Select
        value={value}
        onValueChange={(v) => v != null && onValueChange(v)}
      >
        <SelectTrigger className="h-8 w-full text-[13px]">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>{children}</SelectContent>
      </Select>
    </div>
  );
}
