import { useState } from "react";
import {
  Link,
  Navigate,
  useLocation,
  useNavigate,
  useParams,
  useSearchParams,
} from "react-router-dom";
import {
  IconCopy,
  IconKey,
  IconPlayerPlay,
  IconTrash,
} from "@tabler/icons-react";
import { AppShell } from "@/components/AppShell";
import { Badge } from "@/components/ui/badge";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";
import { Button } from "@/components/ui/button";
import { LinearTriggerForm } from "@/components/triggers/LinearTriggerForm";
import { ScheduleForm, formatWhen } from "@/components/triggers/ScheduleForm";
import { TriggerKindIcon } from "@/components/triggers/TriggerKindIcon";
import { WebhookTriggerForm } from "@/components/triggers/WebhookTriggerForm";
import { isAdminOf, useAuthStatus } from "@/lib/auth";
import {
  useDeleteSchedule,
  useRunScheduleNow,
  useSchedules,
} from "@/lib/schedules";
import {
  type RunTrigger,
  type TriggerKind,
  type TriggerState,
  describeWhen,
  kindMeta,
  triggerState,
  useDeleteTrigger,
  useRotateSecret,
  useRunTrigger,
  useTriggerSystem,
} from "@/lib/triggers";

const LIST = "/triggers";

/** Router state a create hands to the trigger's page: the secret, once. */
interface CreatedState {
  secret?: string | null;
}

/** The app frame with a `Triggers / <page>` trail. */
function TriggerShell({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <AppShell
      title={
        <Breadcrumb>
          <BreadcrumbList className="sm:gap-1.5">
            <BreadcrumbItem>
              <BreadcrumbLink render={<Link to={LIST} />}>
                Triggers
              </BreadcrumbLink>
            </BreadcrumbItem>
            <BreadcrumbSeparator />
            <BreadcrumbItem className="min-w-0">
              <BreadcrumbPage className="truncate font-medium text-foreground">
                {title}
              </BreadcrumbPage>
            </BreadcrumbItem>
          </BreadcrumbList>
        </Breadcrumb>
      }
    >
      <div className="mx-auto flex max-w-3xl flex-col gap-5 p-6">
        {children}
      </div>
    </AppShell>
  );
}

const STATE_VARIANT: Record<TriggerState, "success" | "secondary" | "outline"> =
  { on: "success", idle: "secondary", off: "outline" };

function TriggerHeader({
  kind,
  title,
  subtitle,
  state,
}: {
  kind: TriggerKind;
  title: string;
  subtitle?: React.ReactNode;
  state?: TriggerState;
}) {
  return (
    <div className="flex items-start gap-3">
      <TriggerKindIcon kind={kind} className="mt-0.5 size-6" />
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="text-lg font-semibold">{title}</h1>
          {state && <Badge variant={STATE_VARIANT[state]}>{state}</Badge>}
        </div>
        <div className="text-sm text-muted-foreground">
          {kindMeta(kind).label} trigger
          {subtitle && <> · {subtitle}</>}
        </div>
      </div>
    </div>
  );
}

// ── New ────────────────────────────────────────────────────────────────────

/** `/triggers/new/:kind` */
export function NewTriggerPage() {
  const { kind } = useParams();
  const [params] = useSearchParams();
  const navigate = useNavigate();
  const admin = isAdminOf(useAuthStatus().data);
  const project = params.get("project") ?? undefined;
  const opened = (t: RunTrigger, secret: string | null) =>
    navigate(`/triggers/${encodeURIComponent(t.id)}`, {
      replace: true,
      state: { secret } satisfies CreatedState,
    });

  if (kind === "linear") {
    return (
      <TriggerShell title="New Linear trigger">
        <TriggerHeader kind="linear" title="New Linear trigger" />
        <LinearTriggerForm
          initialProject={project}
          onSaved={(t) => opened(t, null)}
          onCancel={() => navigate(LIST)}
        />
      </TriggerShell>
    );
  }
  if (kind === "webhook") {
    return (
      <TriggerShell title="New webhook">
        <TriggerHeader kind="webhook" title="New webhook" />
        {!admin && <AdminNote what="create webhooks" />}
        <WebhookTriggerForm
          initialProject={project}
          readOnly={!admin}
          onSaved={opened}
          onCancel={() => navigate(LIST)}
        />
      </TriggerShell>
    );
  }
  if (kind === "schedule") {
    return (
      <TriggerShell title="New schedule">
        <TriggerHeader kind="schedule" title="New schedule" />
        {admin ? (
          <ScheduleForm
            schedule={null}
            initialProject={project}
            readOnly={false}
            onSaved={() => navigate(LIST)}
            onCancel={() => navigate(LIST)}
          />
        ) : (
          <AdminNote what="create schedules" />
        )}
      </TriggerShell>
    );
  }
  return <Navigate to={LIST} replace />;
}

// ── Linear triggers and webhooks ───────────────────────────────────────────

/** `/triggers/:id` */
export function RunTriggerPage() {
  const { id = "" } = useParams();
  const location = useLocation();
  const navigate = useNavigate();
  const trigger = useRunTrigger(id);
  const system = useTriggerSystem();
  const admin = isAdminOf(useAuthStatus().data);
  const remove = useDeleteTrigger();
  // A webhook's secret, straight after creating it — the one time it is shown.
  const [secret, setSecret] = useState(
    (location.state as CreatedState | null)?.secret ?? null,
  );

  const t = trigger.data;
  const row = t && { kind: t.kind, key: t.id, trigger: t };
  // Changing a webhook is an administrator's call; a Linear trigger anyone's.
  const canChange = t?.kind === "linear" || admin;

  return (
    <TriggerShell title={t?.name ?? "Trigger"}>
      {trigger.isLoading ? (
        <p className="text-xs text-muted-foreground">Loading…</p>
      ) : !t || !row ? (
        <NotFound what="this trigger" />
      ) : (
        <>
          <TriggerHeader
            kind={t.kind}
            title={t.name}
            state={triggerState(row, system.data ?? "classic")}
            subtitle={
              <>
                {describeWhen(row)} → starts{" "}
                <span className="font-mono">{t.workflow}</span> in{" "}
                <span className="font-mono">{t.project}</span>
              </>
            }
          />

          <LastFired trigger={t} />

          {t.kind === "linear" && system.data === "classic" && (
            <div className="rounded-md border border-border bg-muted/40 px-3 py-2 text-[12px]">
              <span className="font-medium">Idle for now.</span> This harness
              still starts Linear runs from each project&rsquo;s classic
              bindings, so this trigger won&rsquo;t fire until you switch under{" "}
              <Link to="/settings/general" className="underline">
                Settings → General
              </Link>
              .
            </div>
          )}

          {t.kind === "webhook" && (
            <WebhookEndpoint
              trigger={t}
              secret={secret}
              onSecret={setSecret}
              admin={admin}
            />
          )}

          {!canChange && <AdminNote what="change webhooks" />}

          {t.kind === "linear" ? (
            <LinearTriggerForm
              key={t.updated_at}
              trigger={t}
              onSaved={() => navigate(LIST)}
              onCancel={() => navigate(LIST)}
            />
          ) : (
            <WebhookTriggerForm
              key={t.updated_at}
              trigger={t}
              readOnly={!admin}
              onSaved={() => navigate(LIST)}
              onCancel={() => navigate(LIST)}
            />
          )}

          {canChange && (
            <DangerZone
              busy={remove.isPending}
              error={remove.error?.message}
              label="Delete trigger"
              note={
                t.kind === "webhook"
                  ? "Its URL stops working at once. Runs it already started are kept."
                  : "Runs it already started are kept."
              }
              confirmText={`Delete the trigger “${t.name}”?`}
              onDelete={() =>
                remove.mutate(t.id, { onSuccess: () => navigate(LIST) })
              }
            />
          )}
        </>
      )}
    </TriggerShell>
  );
}

/** When the trigger last started a run, or why it last couldn't. */
function LastFired({ trigger: t }: { trigger: RunTrigger }) {
  return (
    <div className="flex flex-wrap gap-x-4 gap-y-1 text-[12px] text-muted-foreground">
      <span>
        Last fired:{" "}
        {t.last_fired_at && t.last_run_id ? (
          <Link to={`/runs/${t.last_run_id}`} className="underline">
            {formatWhen(t.last_fired_at)}
          </Link>
        ) : (
          "never"
        )}
      </span>
      {t.last_error && (
        <span className="text-destructive">
          Last attempt failed: {t.last_error}
        </span>
      )}
      {t.shadowed_by && (
        <span className="text-destructive">
          Not firing:{" "}
          <Link
            to={`/triggers/${encodeURIComponent(t.shadowed_by.id)}`}
            className="underline"
          >
            {t.shadowed_by.name}
          </Link>{" "}
          starts runs from the same Linear status. Remove one of the two, and
          let a route node in the other&rsquo;s workflow choose what to run.
        </span>
      )}
    </div>
  );
}

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Button
      type="button"
      variant="outline"
      size="sm"
      onClick={() => {
        void navigator.clipboard?.writeText(text).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        });
      }}
    >
      <IconCopy /> {copied ? "Copied" : "Copy"}
    </Button>
  );
}

/** Where deliveries go, how to sign them, and the secret — shown once. */
function WebhookEndpoint({
  trigger: t,
  secret,
  onSecret,
  admin,
}: {
  trigger: Extract<RunTrigger, { kind: "webhook" }>;
  secret: string | null;
  onSecret: (secret: string) => void;
  admin: boolean;
}) {
  const rotate = useRotateSecret();
  return (
    <section className="flex flex-col gap-3 rounded-md border border-border p-4">
      <div className="flex flex-col gap-1">
        <span className="text-[11px] font-medium text-muted-foreground">
          Payload URL
        </span>
        {t.url ? (
          <div className="flex items-center gap-2">
            <code className="min-w-0 flex-1 truncate rounded bg-muted px-2 py-1 font-mono text-[12px]">
              {t.url}
            </code>
            <CopyButton text={t.url} />
          </div>
        ) : (
          <span className="text-[12px] text-destructive">
            This harness has no public URL, so nothing outside can reach it. Set
            one under{" "}
            <Link to="/settings/general" className="underline">
              Settings → General
            </Link>
            .
          </span>
        )}
        <span className="text-[11px] text-muted-foreground">
          POST to it with a JSON body.{" "}
          {t.config.auth === "signature"
            ? "Sign the body with the secret (HMAC-SHA256) and send it as X-Hub-Signature-256: sha256=<hex>. For GitHub, choose content type application/json and paste the secret into the webhook's Secret field."
            : "Send the secret as Authorization: Bearer <secret>, an X-Harness-Token header, or ?token=<secret> on the URL."}
        </span>
      </div>

      {secret ? (
        <div className="flex flex-col gap-1 rounded-md border border-primary/40 bg-primary/5 p-3">
          <span className="text-[12px] font-medium">
            The secret — copy it now. It won&rsquo;t be shown again.
          </span>
          <div className="flex items-center gap-2">
            <code className="min-w-0 flex-1 truncate rounded bg-muted px-2 py-1 font-mono text-[12px]">
              {secret}
            </code>
            <CopyButton text={secret} />
          </div>
        </div>
      ) : (
        admin && (
          <div className="flex flex-wrap items-center gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={rotate.isPending}
              onClick={() => {
                if (
                  window.confirm(
                    "Replace the secret? Deliveries signed with the old one are refused from now on.",
                  )
                ) {
                  rotate.mutate(t.id, { onSuccess: (r) => onSecret(r.secret) });
                }
              }}
            >
              <IconKey /> New secret
            </Button>
            <span className="text-[11px] text-muted-foreground">
              The secret is stored encrypted and shown only when it is made.
            </span>
            {rotate.isError && (
              <span className="text-[11px] text-destructive">
                {rotate.error.message}
              </span>
            )}
          </div>
        )
      )}
    </section>
  );
}

// ── Schedules ──────────────────────────────────────────────────────────────

/** `/triggers/schedule/:id` */
export function ScheduleTriggerPage() {
  const { id = "" } = useParams();
  const schedules = useSchedules();
  const admin = isAdminOf(useAuthStatus().data);
  const run = useRunScheduleNow();
  const remove = useDeleteSchedule();
  const navigate = useNavigate();
  const schedule = schedules.data?.schedules.find((s) => s.id === id);

  return (
    <TriggerShell title={schedule?.name ?? "Schedule"}>
      {schedules.isLoading ? (
        <p className="text-xs text-muted-foreground">Loading…</p>
      ) : schedules.isError ? (
        <p className="text-sm text-destructive">{schedules.error.message}</p>
      ) : !schedule ? (
        <NotFound what="this schedule" />
      ) : (
        <>
          <TriggerHeader
            kind="schedule"
            title={schedule.name}
            state={schedule.enabled ? "on" : "off"}
            subtitle={
              <>
                starts <span className="font-mono">{schedule.workflow}</span> in{" "}
                <span className="font-mono">{schedule.project}</span>
              </>
            }
          />
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[12px] text-muted-foreground">
            <span>
              {describeWhen({
                kind: "schedule",
                key: schedule.id,
                schedule,
              })}
            </span>
            <span>
              Next:{" "}
              {schedule.enabled ? formatWhen(schedule.next_run_at) : "paused"}
            </span>
            {schedule.last_run_id && (
              <span>
                Last:{" "}
                <Link
                  to={`/runs/${schedule.last_run_id}`}
                  className="underline"
                >
                  {formatWhen(schedule.last_run_at)}
                </Link>
              </span>
            )}
            {admin && (
              <Button
                size="sm"
                variant="outline"
                disabled={run.isPending}
                onClick={() => run.mutate(schedule.id)}
                title="Start a run now; the schedule itself is unchanged"
              >
                <IconPlayerPlay /> Run now
              </Button>
            )}
            {run.data && (
              <Link to={`/runs/${run.data.run_id}`} className="underline">
                Started — open the run
              </Link>
            )}
            {run.isError && (
              <span className="text-destructive">{run.error.message}</span>
            )}
          </div>
          {!admin && <AdminNote what="change schedules" />}
          <ScheduleForm
            key={schedule.updated_at}
            schedule={schedule}
            readOnly={!admin}
            onSaved={() => navigate(LIST)}
            onCancel={() => navigate(LIST)}
          />
          {admin && (
            <DangerZone
              busy={remove.isPending}
              error={remove.error?.message}
              label="Delete schedule"
              note="Runs it already started are kept."
              confirmText={`Delete the schedule “${schedule.name}”?`}
              onDelete={() =>
                remove.mutate(schedule.id, { onSuccess: () => navigate(LIST) })
              }
            />
          )}
        </>
      )}
    </TriggerShell>
  );
}

// ── Shared ─────────────────────────────────────────────────────────────────

function AdminNote({ what }: { what: string }) {
  return (
    <p className="text-[12px] text-muted-foreground">
      Only administrators {what}.
    </p>
  );
}

function NotFound({ what }: { what: string }) {
  return (
    <div className="flex flex-col items-start gap-3">
      <p className="text-sm text-muted-foreground">
        There is no {what}. It may have been deleted.
      </p>
      <Button size="sm" variant="outline" render={<Link to={LIST} />}>
        All triggers
      </Button>
    </div>
  );
}

function DangerZone({
  label,
  note,
  confirmText,
  busy,
  error,
  onDelete,
}: {
  label: string;
  note: string;
  confirmText: string;
  busy: boolean;
  error?: string;
  onDelete: () => void;
}) {
  return (
    <section className="flex flex-col gap-2 rounded-md border border-destructive/40 p-4">
      <h2 className="text-sm font-semibold">{label}</h2>
      <p className="text-xs text-muted-foreground">{note}</p>
      <div className="flex items-center gap-2">
        <Button
          variant="destructive"
          size="sm"
          disabled={busy}
          onClick={() => {
            if (window.confirm(confirmText)) onDelete();
          }}
        >
          <IconTrash /> {busy ? "Deleting…" : "Delete"}
        </Button>
        {error && <span className="text-xs text-destructive">{error}</span>}
      </div>
    </section>
  );
}
