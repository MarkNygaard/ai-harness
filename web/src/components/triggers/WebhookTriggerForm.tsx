import { useState } from "react";
import { IconPlus, IconTrash } from "@tabler/icons-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import {
  type RunTrigger,
  type WebhookAuth,
  type WebhookTriggerConfig,
  useSaveTrigger,
} from "@/lib/triggers";
import { Field, OptionSelect, ProjectSelect, WorkflowSelect } from "./fields";

type WebhookTrigger = Extract<RunTrigger, { kind: "webhook" }>;

/** A filter row as typed: values comma-separated until saved. */
interface FilterRow {
  path: string;
  values: string;
}

const AUTH_OPTIONS: { value: WebhookAuth; label: string }[] = [
  { value: "signature", label: "Signature (HMAC-SHA256, GitHub-style)" },
  { value: "token", label: "Token (the secret as a bearer token)" },
];

/**
 * A webhook trigger's settings: what a delivery must look like, and what the
 * run it starts is asked to do. The URL and secret are shown by the page, not
 * here — the secret only once.
 */
export function WebhookTriggerForm({
  trigger,
  initialProject,
  readOnly,
  onSaved,
  onCancel,
}: {
  trigger?: WebhookTrigger;
  initialProject?: string;
  /** Only administrators change webhooks: they open a public endpoint. */
  readOnly: boolean;
  onSaved: (saved: RunTrigger, secret: string | null) => void;
  onCancel: () => void;
}) {
  const c = trigger?.config;
  const [name, setName] = useState(trigger?.name ?? "");
  const [project, setProject] = useState(
    trigger?.project ?? initialProject ?? "",
  );
  const [workflow, setWorkflow] = useState(trigger?.workflow ?? "");
  const [auth, setAuth] = useState<WebhookAuth>(c?.auth ?? "signature");
  const [filters, setFilters] = useState<FilterRow[]>(
    (c?.filters ?? []).map((f) => ({
      path: f.path,
      values: f.equals.join(", "),
    })),
  );
  const [template, setTemplate] = useState(c?.task_template ?? "");
  const [deliveryHeader, setDeliveryHeader] = useState(
    c?.delivery_header ?? "",
  );
  const [baseBranch, setBaseBranch] = useState(c?.base_branch ?? "");
  const [enabled, setEnabled] = useState(trigger?.enabled ?? true);
  const save = useSaveTrigger();

  const setFilter = (i: number, patch: Partial<FilterRow>) =>
    setFilters((fs) => fs.map((f, j) => (j === i ? { ...f, ...patch } : f)));

  const cleaned = filters
    .map((f) => ({
      path: f.path.trim(),
      equals: f.values
        .split(",")
        .map((v) => v.trim())
        .filter(Boolean),
    }))
    .filter((f) => f.path || f.equals.length);
  const badFilter = cleaned.find(
    (f) =>
      !(f.path.startsWith("body.") || f.path.startsWith("headers.")) ||
      f.equals.length === 0,
  );

  const canSave =
    !readOnly &&
    !!name.trim() &&
    !!project &&
    !!workflow &&
    !badFilter &&
    !save.isPending;

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!canSave) return;
    const config: WebhookTriggerConfig = {
      auth,
      filters: cleaned,
      task_template: template,
      delivery_header: deliveryHeader.trim() || null,
      base_branch: baseBranch.trim() || null,
    };
    save.mutate(
      {
        id: trigger?.id,
        body: {
          name: name.trim(),
          kind: "webhook",
          project,
          workflow,
          enabled,
          config,
        },
      },
      { onSuccess: (res) => onSaved(res.trigger, res.secret) },
    );
  };

  return (
    <form className="flex flex-col gap-4" onSubmit={submit}>
      <fieldset disabled={readOnly} className="flex flex-col gap-4">
        <Field label="Name" hint="Also titles each run, with the time.">
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Review new pull requests"
            autoFocus={!trigger}
          />
        </Field>

        <div className="grid gap-4 sm:grid-cols-2">
          <Field label="Project">
            <ProjectSelect
              value={project}
              onChange={setProject}
              disabled={readOnly}
            />
          </Field>
          <Field label="Workflow">
            <WorkflowSelect
              value={workflow}
              onChange={setWorkflow}
              disabled={readOnly}
            />
          </Field>
        </div>

        <Field
          label="How deliveries prove they're genuine"
          hint={
            auth === "signature"
              ? "The sender signs the body with the secret and sends X-Hub-Signature-256: sha256=<hex>. GitHub does this when you paste the secret into its webhook settings."
              : "The sender sends the secret as Authorization: Bearer <secret>, an X-Harness-Token header, or ?token= on the URL. Use it only when a sender can't sign."
          }
        >
          <OptionSelect
            value={auth}
            onChange={(v) => setAuth(v as WebhookAuth)}
            disabled={readOnly}
            placeholder="Choose"
            options={AUTH_OPTIONS}
          />
        </Field>

        <Field
          label="Only when"
          hint={
            badFilter
              ? "Each condition needs a path starting with body. or headers., and at least one value."
              : "Every condition must hold, or the delivery is acknowledged and ignored. A value list matches any of its values."
          }
        >
          <div className="flex flex-col gap-2">
            {filters.map((f, i) => (
              <div key={i} className="flex items-center gap-2">
                <Input
                  value={f.path}
                  onChange={(e) => setFilter(i, { path: e.target.value })}
                  placeholder="headers.x-github-event"
                  className="flex-1 font-mono"
                />
                <span className="text-[12px] text-muted-foreground">is</span>
                <Input
                  value={f.values}
                  onChange={(e) => setFilter(i, { values: e.target.value })}
                  placeholder="pull_request"
                  className="flex-1 font-mono"
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  aria-label="Remove condition"
                  onClick={() =>
                    setFilters((fs) => fs.filter((_, j) => j !== i))
                  }
                >
                  <IconTrash />
                </Button>
              </div>
            ))}
            <div>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() =>
                  setFilters((fs) => [...fs, { path: "", values: "" }])
                }
              >
                <IconPlus /> Add condition
              </Button>
            </div>
          </div>
        </Field>

        <Field
          label="Task"
          hint={
            <>
              What the run is asked to do. Fill in parts of the delivery with{" "}
              <code className="font-mono">
                {"{{body.pull_request.html_url}}"}
              </code>{" "}
              or{" "}
              <code className="font-mono">{"{{headers.x-github-event}}"}</code>.
              Left empty, the run gets the whole payload.
            </>
          }
        >
          <Textarea
            value={template}
            onChange={(e) => setTemplate(e.target.value)}
            placeholder="Review the pull request {{body.pull_request.html_url}}"
            className="min-h-24 font-mono text-[12px]"
          />
        </Field>

        <div className="grid gap-4 sm:grid-cols-2">
          <Field
            label="Delivery id header"
            hint="A retry with the same id starts nothing. Empty: X-GitHub-Delivery, X-Request-Id and other common ones."
          >
            <Input
              value={deliveryHeader}
              onChange={(e) => setDeliveryHeader(e.target.value)}
              placeholder="auto"
              className="font-mono"
            />
          </Field>
          <Field label="Base branch" hint="Empty: the project's own.">
            <Input
              value={baseBranch}
              onChange={(e) => setBaseBranch(e.target.value)}
              placeholder="main"
              className="font-mono"
            />
          </Field>
        </div>

        <label className="flex items-center gap-2 text-sm">
          <Switch
            checked={enabled}
            onCheckedChange={setEnabled}
            disabled={readOnly}
          />
          Enabled
        </label>
      </fieldset>

      {save.isError && (
        <p className="text-[12px] text-destructive">{save.error.message}</p>
      )}
      {!readOnly && (
        <div className="flex items-center gap-2">
          <Button type="submit" size="sm" disabled={!canSave}>
            {save.isPending ? "Saving…" : trigger ? "Save" : "Create webhook"}
          </Button>
          <Button type="button" size="sm" variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
        </div>
      )}
    </form>
  );
}
