import React, { useState } from "react";
import { Plus, X } from "lucide-react";
import { Markdown, ViewToggle } from "@/components/Markdown";
import type {
  Catalog,
  EditorLinear,
  EditorNode,
  EditorRoute,
  NodeKindId,
} from "@/types/authoring";
import { emptyNode, nodeKind } from "@/lib/workflow-yaml";
import { useCategories } from "@/lib/categories";
import { SaveStepButton } from "./SaveStepDialog";
import { McpPicker } from "./McpPicker";
import { isMetered } from "@/lib/models";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Button } from "@/components/ui/button";
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
  "linear",
  "route",
] as const satisfies readonly (keyof EditorNode)[];

/** Right drawer: edit the selected node's id, kind, body, and AI options. */
export function PropertiesDrawer({
  node,
  catalog,
  workflowProvider,
  workflowModel,
  onChange,
  onClose,
}: {
  node: EditorNode;
  catalog: Catalog | undefined;
  /**
   * What the workflow itself sets, for a node that does not override it.
   *
   * A node inheriting the workflow's agent and model is the normal case —
   * `judge-ab` deliberately has none of its own, so the judge model stays
   * constant across both arms of a comparison. But "inherits" and "unset" then
   * look identical in this panel, and a workflow that plainly runs on Opus
   * reads as having no model at all. Naming what is inherited is the whole
   * difference.
   */
  workflowProvider?: string;
  workflowModel?: string;
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

  // Only these dispatch to an agent, so only these have an agent, a model, an
  // effort or a context to set. The executor already knows this — a bash,
  // script, approval or cancel node is given no provider even when the workflow
  // has one — so offering the fields here promised a setting that would be
  // ignored, and a Shell step showing "Agent" reads as though it runs one.
  // A route asks a model only when it has a prompt; rules alone run none.
  const isAgentStep =
    kind === "prompt" ||
    kind === "command" ||
    kind === "loop" ||
    (kind === "route" && !!node.route?.prompt);

  const provider = node.provider ?? "";
  const providerInfo = catalog?.providers.find((p) => p.id === provider);
  const providerModels = providerInfo?.models ?? [];
  // Models this agent's installed CLI is too old for. Shown, but not
  // selectable, so the list says which update would unlock them.
  const unavailableModels = providerInfo?.unavailable ?? [];
  // Always include the node's current model, so a value not in the catalog
  // (a bundled workflow's model, or one whose agent isn't connected) still shows.
  // One held back for its CLI version already shows, as unavailable.
  const modelOptions =
    node.model &&
    !providerModels.includes(node.model) &&
    !unavailableModels.some((u) => u.id === node.model)
      ? [node.model, ...providerModels]
      : providerModels;
  // What this step will actually run on, following both inheritances — the
  // metered warning below has to be true of the effective pair, not just of an
  // explicit override. A node that inherits Cursor + `claude-opus-5` from the
  // workflow bills the same as one that names them.
  const effProvider = provider || workflowProvider;
  const effModel = node.model ?? workflowModel;
  const effProviderInfo = catalog?.providers.find((p) => p.id === effProvider);
  const effUnavailable = effProviderInfo?.unavailable?.find(
    (u) => u.id === effModel,
  );

  return (
    <div className="flex w-1/3 min-w-[20rem] flex-none flex-col border-l border-border bg-card">
      <div className="flex items-center justify-between border-b border-border px-4 py-3">
        <span className="text-sm font-semibold">Step settings</span>
        <div className="flex items-center gap-1">
          <SaveStepButton node={node} />
          <button
            type="button"
            onClick={onClose}
            className="rounded p-1 hover:bg-secondary"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
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
          <PromptField
            value={node.prompt ?? ""}
            onChange={(v) => set({ prompt: v })}
          />
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
        {kind === "linear" && (
          <LinearBody
            value={node.linear ?? {}}
            onChange={(linear) => set({ linear })}
          />
        )}
        {kind === "route" && node.route && (
          <RouteBody
            id={node.id}
            value={node.route}
            onChange={(route) => set({ route })}
          />
        )}

        <Field label="When (condition)">
          <Input
            className="font-mono"
            value={node.when ?? ""}
            onChange={(e) => set({ when: e.target.value || undefined })}
            placeholder="$classify.output.type == 'BUG'"
          />
        </Field>

        {isAgentStep && (
          <>
            <div className="mt-1 border-t border-border pt-3 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
              AI options
            </div>

            <SelectField
              label="Agent"
              value={provider || DEFAULT_SENTINEL}
              onValueChange={(v) =>
                set({ provider: v === DEFAULT_SENTINEL ? undefined : v })
              }
            >
              <SelectItem value={DEFAULT_SENTINEL}>
                {inheritedLabel(
                  catalog?.providers.find((p) => p.id === workflowProvider)
                    ?.label ?? workflowProvider,
                )}
              </SelectItem>
              {(catalog?.providers ?? []).map((p) => (
                <SelectItem key={p.id} value={p.id}>
                  {p.label}
                </SelectItem>
              ))}
            </SelectField>
            {/* Field + its note in one tight column, so the note reads as
                belonging to the model rather than floating between fields. */}
            <div className="flex flex-col gap-1">
              <SelectField
                label="Model"
                value={node.model ?? DEFAULT_SENTINEL}
                onValueChange={(v) =>
                  set({ model: v === DEFAULT_SENTINEL ? undefined : v })
                }
              >
                <SelectItem value={DEFAULT_SENTINEL}>
                  {inheritedLabel(workflowModel)}
                </SelectItem>
                {modelOptions.map((m) => (
                  <SelectItem key={m} value={m}>
                    {/* An element rather than a bare string, which also keeps
                        `optionLabels` from registering a label for it — so the
                        closed trigger shows the plain id and not the badge. */}
                    <span className="flex items-center gap-2">
                      {m}
                      {isMetered(provider, m) && <MeteredBadge />}
                    </span>
                  </SelectItem>
                ))}
                {unavailableModels.map((u) => (
                  <SelectItem key={u.id} value={u.id} disabled>
                    <span className="flex items-center gap-2">
                      {u.id}
                      <span className="text-[11px] text-muted-foreground">
                        needs {providerInfo?.label ?? provider} {u.min_cli}
                      </span>
                    </span>
                  </SelectItem>
                ))}
              </SelectField>
              {effUnavailable && (
                <p className="text-[11px] text-destructive">
                  {effProviderInfo?.label ?? effProvider}{" "}
                  {effUnavailable.installed} is installed; this model needs{" "}
                  {effUnavailable.min_cli}. Update the CLI or pick another
                  model.
                </p>
              )}
              {/* The badge above is only visible while the list is open, and
                  this is the one property worth still knowing once it is
                  closed. */}
              {isMetered(effProvider, effModel) && (
                <p className="text-[11px] text-muted-foreground">
                  Billed on top of the subscription, at this model&rsquo;s API
                  price.
                </p>
              )}
            </div>
            {/* Hidden for an agent with no effort setting (cursor-agent, the
                direct API) — unless the step already sets one, which stays
                visible so it can be cleared; validation warns about it. */}
            {(effProviderInfo?.takes_effort !== false || node.effort) && (
              <SelectField
                label="Effort"
                value={node.effort ?? DEFAULT_SENTINEL}
                onValueChange={(v) =>
                  set({
                    effort:
                      v === DEFAULT_SENTINEL
                        ? undefined
                        : (v as EditorNode["effort"]),
                  })
                }
              >
                <SelectItem value={DEFAULT_SENTINEL}>
                  (agent default)
                </SelectItem>
                {EFFORT_LEVELS.map((e) => (
                  <SelectItem key={e} value={e}>
                    {e}
                  </SelectItem>
                ))}
              </SelectField>
            )}
            {/* A route's model only picks a way out; it gets no tools. */}
            {kind !== "route" && (
              <McpPicker
                value={node.mcp ?? []}
                provider={effProvider}
                onChange={(mcp) => set({ mcp })}
              />
            )}
          </>
        )}
        {/* Trigger rule is about this step's dependencies and applies to every
            kind; context is the agent's session and does not. */}
        <div className={isAgentStep ? "grid grid-cols-2 gap-2" : undefined}>
          {isAgentStep && (
            <SelectField
              label="Context"
              value={node.context ?? "shared"}
              onValueChange={(v) =>
                set({ context: v as EditorNode["context"] })
              }
            >
              {(catalog?.context_modes ?? ["fresh", "shared"]).map((c) => (
                <SelectItem key={c} value={c}>
                  {c}
                </SelectItem>
              ))}
            </SelectField>
          )}
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

/**
 * The node's prompt: a textarea with an Edit/Preview switch so long prompts
 * with markdown (headings, lists, tables) can be read formatted via the same
 * renderer used elsewhere in the app.
 */
function PromptField({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}) {
  const [preview, setPreview] = useState(false);
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center justify-between">
        <span className="text-[11px] font-medium text-muted-foreground">
          Prompt
        </span>
        <ViewToggle
          value={preview}
          onChange={setPreview}
          renderedLabel="Preview"
          rawLabel="Edit"
        />
      </div>
      {preview ? (
        <div className="min-h-35 rounded-md border border-input bg-transparent p-2">
          {value.trim() ? (
            <Markdown>{value}</Markdown>
          ) : (
            <p className="text-[12px] italic text-muted-foreground">
              Nothing to preview.
            </p>
          )}
        </div>
      ) : (
        <Textarea
          rows={6}
          value={value}
          onChange={(e) => onChange(e.target.value)}
        />
      )}
    </div>
  );
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

/** Reasoning-effort levels (claude, codex, omp), low → max. */
const EFFORT_LEVELS = ["low", "medium", "high", "xhigh", "max"] as const;

/** A labelled shadcn Select (replaces the native `<select>` for a styled popup).
 *  Not wrapped in a `<label>` — the trigger is a button. */
/**
 * What the workflow's own setting is, for the "inherit" option.
 *
 * Named rather than left as a bare "(workflow default)": a reader looking at
 * `judge-ab` sees a step with no agent and no model and concludes none is set,
 * when the workflow plainly runs on Claude/Opus and the step is inheriting it
 * on purpose.
 */
function inheritedLabel(value: string | undefined): string {
  return value ? `Workflow default (${value})` : "Workflow default";
}

/**
 * This model is not covered by the agent's subscription.
 *
 * Only the exception is marked. Most models in most lists are included, so a
 * badge on every row would be noise that says nothing — it is the Cursor list,
 * where four included models sit above nine metered ones with nothing in the
 * ids to tell them apart, that this exists for.
 */
function MeteredBadge() {
  return (
    <span
      className="shrink-0 rounded-sm bg-muted px-1 py-px text-[10px] font-medium text-muted-foreground"
      title="Not included in the subscription — charged per token at this model's API price."
    >
      API price
    </span>
  );
}

/**
 * The text each option shows, read off the options themselves.
 *
 * Base UI's `Select.Value` renders the **raw value** unless given a formatter,
 * so every field here whose label differs from its value displayed the value:
 * the agent read `claude` after you picked "Claude Code", the type read
 * `prompt` rather than "Agent step", and an inherited setting read
 * `__default__`. Deriving the map from the items means a field cannot be added
 * with the bug still in it, which listing the labels by hand at each call site
 * would not prevent.
 *
 * A non-string label (an element) has no text to reuse, so those fall back to
 * the value rather than rendering nothing.
 */
export function optionLabels(
  children: React.ReactNode,
): Record<string, string> {
  const out: Record<string, string> = {};
  for (const child of React.Children.toArray(children)) {
    if (!React.isValidElement(child)) continue;
    const props = child.props as { value?: unknown; children?: unknown };
    if (typeof props.value === "string" && typeof props.children === "string") {
      out[props.value] = props.children;
    }
  }
  return out;
}

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
  const labels = optionLabels(children);
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
          <SelectValue>
            {(v: string | null) => (v != null && labels[v]) || v || ""}
          </SelectValue>
        </SelectTrigger>
        <SelectContent>{children}</SelectContent>
      </Select>
    </div>
  );
}

/** Which `linear.*` key this node carries. */
type LinearActionId = "move_state" | "comment" | "add_labels" | "remove_labels";

const LINEAR_ACTIONS: { id: LinearActionId; label: string; help: string }[] = [
  {
    id: "move_state",
    label: "Move status",
    help: "A status name as it reads in Linear, resolved against the issue's own team.",
  },
  {
    id: "comment",
    label: "Comment",
    help: "Posted on the issue. Takes variables, so $TASK_TITLE or an upstream node's output works here.",
  },
  {
    id: "add_labels",
    label: "Add labels",
    help: "Existing label names, comma-separated. A name that is not already a label is refused rather than created.",
  },
  {
    id: "remove_labels",
    label: "Remove labels",
    help: "Comma-separated. Removing one the issue does not have is a no-op, not an error.",
  },
];

/**
 * The `linear:` body — one action, matching the parser's one-of shape.
 *
 * Switching action replaces the key rather than adding one, because a node
 * carrying two would be rejected at parse time with "conflicting bodies".
 */
function LinearBody({
  value,
  onChange,
}: {
  value: EditorLinear;
  onChange: (v: EditorLinear) => void;
}) {
  const active: LinearActionId =
    LINEAR_ACTIONS.find((a) => value[a.id] !== undefined)?.id ?? "move_state";
  const meta = LINEAR_ACTIONS.find((a) => a.id === active)!;
  const isLabels = active === "add_labels" || active === "remove_labels";
  const text = isLabels
    ? ((value[active] as string[] | undefined) ?? []).join(", ")
    : ((value[active] as string | undefined) ?? "");

  const setText = (next: string) =>
    onChange({
      [active]: isLabels
        ? next
            .split(",")
            .map((s) => s.trim())
            .filter(Boolean)
        : next,
    });

  return (
    <>
      <SelectField
        label="Linear action"
        value={active}
        onValueChange={(id) =>
          // Replace, never merge: a node carrying two linear keys is rejected
          // at parse time as conflicting bodies.
          onChange(
            id === "add_labels" || id === "remove_labels"
              ? { [id]: [] }
              : { [id]: "" },
          )
        }
      >
        {LINEAR_ACTIONS.map((a) => (
          <SelectItem key={a.id} value={a.id}>
            {a.label}
          </SelectItem>
        ))}
      </SelectField>
      <Field label={meta.label}>
        {active === "comment" ? (
          <Textarea
            rows={3}
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="Working on $TASK_TITLE"
          />
        ) : (
          <Input
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder={
              active === "move_state"
                ? "In Progress"
                : "needs-review, automated"
            }
          />
        )}
        <span className="text-[10px] text-muted-foreground">{meta.help}</span>
      </Field>
    </>
  );
}

/** No default: every route's rule false and no model answer fails the step. */
const NO_DEFAULT = "__none__";

/**
 * The `route:` body: the ways out, how each is chosen, and where to go when
 * nothing chooses. Rules are checked in order first, then the prompt (if any)
 * asks a model to pick by the descriptions, then the default.
 */
function RouteBody({
  id,
  value,
  onChange,
}: {
  id: string;
  value: EditorRoute;
  onChange: (v: EditorRoute) => void;
}) {
  const setRoute = (
    i: number,
    patch: Partial<EditorRoute["routes"][number]>,
  ) => {
    const old = value.routes[i];
    const routes = value.routes.map((r, j) =>
      j === i ? { ...r, ...patch } : r,
    );
    // The default follows a rename of the route it names.
    const renamedDefault =
      patch.name !== undefined && value.default === old.name
        ? patch.name
        : value.default;
    onChange({ ...value, routes, default: renamedDefault });
  };
  const remove = (i: number) => {
    const gone = value.routes[i].name;
    onChange({
      ...value,
      routes: value.routes.filter((_, j) => j !== i),
      default: value.default === gone ? undefined : value.default,
    });
  };
  const add = () => {
    const names = new Set(value.routes.map((r) => r.name));
    let n = value.routes.length + 1;
    while (names.has(`route-${n}`)) n++;
    onChange({ ...value, routes: [...value.routes, { name: `route-${n}` }] });
  };

  return (
    <>
      <Field label="Ask a model (optional)">
        <Textarea
          rows={3}
          value={value.prompt ?? ""}
          onChange={(e) =>
            onChange({ ...value, prompt: e.target.value || undefined })
          }
          placeholder="Is this Linear issue an epic or a single task? $ARGUMENTS"
        />
        <span className="text-[10px] text-muted-foreground">
          Asked only when no route&apos;s rule holds. The model picks a route by
          its description.
        </span>
      </Field>

      <div className="flex flex-col gap-2">
        <span className="text-[11px] font-medium text-muted-foreground">
          Routes
        </span>
        {value.routes.map((r, i) => (
          <div
            key={i}
            className="flex flex-col gap-1.5 rounded-md border border-border p-2"
          >
            <div className="flex items-center gap-1.5">
              <Input
                className="font-mono"
                value={r.name}
                onChange={(e) => setRoute(i, { name: e.target.value })}
                placeholder="name"
                aria-label="Route name"
              />
              <Button
                variant="ghost"
                size="icon"
                title={
                  value.routes.length <= 2
                    ? "A route needs at least two ways out"
                    : "Remove route"
                }
                disabled={value.routes.length <= 2}
                onClick={() => remove(i)}
              >
                <X />
              </Button>
            </div>
            <Input
              value={r.description ?? ""}
              onChange={(e) =>
                setRoute(i, { description: e.target.value || undefined })
              }
              placeholder="What belongs here (read by the model)"
              aria-label="Route description"
            />
            <Input
              className="font-mono"
              value={r.when ?? ""}
              onChange={(e) =>
                setRoute(i, { when: e.target.value || undefined })
              }
              placeholder="Rule, e.g. $fetch.output.children > 0"
              aria-label="Route rule"
            />
          </div>
        ))}
        <Button variant="outline" size="sm" onClick={add}>
          <Plus />
          Add route
        </Button>
        <span className="text-[10px] text-muted-foreground">
          Connect a step from a route&apos;s exit on the canvas to put it on
          that route. It then runs when{" "}
          <code className="font-mono">{`$${id || "id"}.output.route`}</code>{" "}
          names it.
        </span>
      </div>

      <SelectField
        label="Default"
        value={value.default ?? NO_DEFAULT}
        onValueChange={(v) =>
          onChange({ ...value, default: v === NO_DEFAULT ? undefined : v })
        }
      >
        <SelectItem value={NO_DEFAULT}>None (fail the step)</SelectItem>
        {value.routes
          .filter((r) => r.name)
          .map((r, i) => (
            <SelectItem key={`${i}-${r.name}`} value={r.name}>
              {r.name}
            </SelectItem>
          ))}
      </SelectField>
    </>
  );
}
