import { useState } from "react";
import {
  Link,
  Navigate,
  useNavigate,
  useParams,
  useSearchParams,
} from "react-router-dom";
import { IconHierarchy2, IconPlayerPlay, IconTrash } from "@tabler/icons-react";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { WiringCheck, EpicSetupForm } from "@/components/projects/EpicSetup";
import {
  AccountSelect,
  BindingForm,
} from "@/components/projects/ProjectLinear";
import { ScheduleForm, formatWhen } from "@/components/triggers/ScheduleForm";
import { TriggerKindIcon } from "@/components/triggers/TriggerKindIcon";
import { isAdminOf, useAuthStatus } from "@/lib/auth";
import { useWorkflowList } from "@/lib/authoring";
import {
  useDeleteLinearSource,
  useLinearConnections,
  useLinearDiscovery,
  useLinearSources,
} from "@/lib/linear";
import { useProjects } from "@/lib/projects";
import {
  useDeleteSchedule,
  useRunScheduleNow,
  useSchedules,
} from "@/lib/schedules";
import { useLinearTriggerHome } from "@/lib/trigger-prefs";
import {
  type TriggerKind,
  describeLinearWhen,
  describeScheduleWhen,
  fromBinding,
  fromSchedule,
  kindMeta,
} from "@/lib/triggers";
import type { DeletedState } from "./TriggersPage";

const LIST = "/triggers";

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

/** Kind icon, the trigger's "when", its state, and what it starts. */
function TriggerHeader({
  kind,
  title,
  workflow,
  project,
  state,
}: {
  kind: TriggerKind;
  title: string;
  workflow?: string;
  project?: string;
  state?: "on" | "dry-run" | "off";
}) {
  return (
    <div className="flex items-start gap-3">
      <TriggerKindIcon kind={kind} className="mt-0.5 size-6" />
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="text-lg font-semibold">{title}</h1>
          {state && (
            <Badge
              variant={
                state === "on"
                  ? "success"
                  : state === "dry-run"
                    ? "secondary"
                    : "outline"
              }
            >
              {state}
            </Badge>
          )}
        </div>
        <div className="text-sm text-muted-foreground">
          {kindMeta(kind).label} trigger
          {workflow && project && (
            <>
              {" "}
              · starts <span className="font-mono">{workflow}</span> in{" "}
              <span className="font-mono">{project}</span>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

function ProjectPicker({
  value,
  onChange,
}: {
  value: string | null;
  onChange: (project: string) => void;
}) {
  const projects = useProjects();
  return (
    <div className="flex flex-col gap-1">
      <span className="text-[11px] font-medium text-muted-foreground">
        Project
      </span>
      <Select value={value} onValueChange={(v) => v != null && onChange(v)}>
        <SelectTrigger className="w-full sm:w-72">
          <SelectValue>
            {(v: string | null) => v || "Choose a project"}
          </SelectValue>
        </SelectTrigger>
        <SelectContent>
          {(projects.data ?? []).map((p) => (
            <SelectItem key={p.name} value={p.name}>
              {p.name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}

/** `/triggers/new/:kind` */
export function NewTriggerPage() {
  const { kind } = useParams();
  if (kind === "linear") return <NewLinearTrigger />;
  if (kind === "schedule") return <NewScheduleTrigger />;
  return <Navigate to={LIST} replace />;
}

// ── Linear ─────────────────────────────────────────────────────────────────

const linearTab = (project: string) =>
  `/settings/projects/${encodeURIComponent(project)}/linear`;

/** Until a usable Linear account exists there is nothing to bind to. */
function useLinearAccounts() {
  const connections = useLinearConnections();
  const accounts = connections.data ?? [];
  return {
    loading: connections.isLoading,
    accounts,
    usable: accounts.some((c) => c.mode !== "none"),
  };
}

function NoLinearAccount() {
  return (
    <div className="rounded-md border border-dashed border-border px-4 py-6 text-center text-sm text-muted-foreground">
      No Linear account is connected.{" "}
      <Link
        to="/settings/integrations"
        className="text-foreground underline underline-offset-2"
      >
        Connect one under Integrations
      </Link>{" "}
      to start runs from Linear issues.
    </div>
  );
}

function NewLinearTrigger() {
  const [params, setParams] = useSearchParams();
  const project = params.get("project");
  const home = useLinearTriggerHome();
  const { loading, accounts, usable } = useLinearAccounts();
  const navigate = useNavigate();

  // This browser manages Linear triggers on the project page.
  if (home === "project" && project) {
    return <Navigate to={linearTab(project)} replace />;
  }

  return (
    <TriggerShell title="New Linear trigger">
      <TriggerHeader kind="linear" title="New Linear trigger" />
      {loading ? (
        <p className="text-xs text-muted-foreground">Loading…</p>
      ) : !usable ? (
        <NoLinearAccount />
      ) : (
        <>
          <ProjectPicker
            value={project}
            onChange={(p) => setParams({ project: p }, { replace: true })}
          />
          {project && (
            <LinearTriggerEditor
              key={project}
              project={project}
              accounts={accounts}
              onDone={() => navigate(LIST)}
            />
          )}
        </>
      )}
    </TriggerShell>
  );
}

/** A new binding's form for one project, or the epic set-up for it. */
function LinearTriggerEditor({
  project,
  accounts,
  onDone,
}: {
  project: string;
  accounts: ReturnType<typeof useLinearAccounts>["accounts"];
  onDone: () => void;
}) {
  const discovery = useLinearDiscovery(project);
  const sources = useLinearSources(project);
  const workflows = useWorkflowList();
  const [epics, setEpics] = useState(false);
  const bound = new Set((sources.data ?? []).map((s) => s.workflow));
  const available = (workflows.data ?? []).filter((w) => !bound.has(w.name));
  const teams = discovery.data?.teams ?? [];

  return (
    <div className="flex flex-col gap-4">
      <AccountSelect project={project} accounts={accounts} />
      {discovery.isError ? (
        <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive">
          {discovery.error.message}
        </div>
      ) : !discovery.data || !sources.data ? (
        <p className="text-xs text-muted-foreground">Loading…</p>
      ) : epics ? (
        <EpicSetupForm
          project={project}
          sources={sources.data}
          teams={teams}
          onDone={onDone}
        />
      ) : available.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          Every workflow already has a Linear trigger in {project}. Open one
          from the list to change it.
        </p>
      ) : (
        <>
          <BindingForm
            project={project}
            workflow=""
            fixedWorkflow={false}
            availableWorkflows={available}
            teams={teams}
            onDone={onDone}
          />
          <div className="border-t border-border pt-4">
            <Button size="sm" variant="outline" onClick={() => setEpics(true)}>
              <IconHierarchy2 /> Set up epics instead
            </Button>
            <p className="mt-1 text-[11px] text-muted-foreground">
              Writes the three triggers that build an epic piece by piece.
            </p>
          </div>
        </>
      )}
    </div>
  );
}

/** `/triggers/linear/:project/:workflow` */
export function LinearTriggerPage() {
  const { project = "", workflow = "" } = useParams();
  const home = useLinearTriggerHome();
  const { loading, accounts, usable } = useLinearAccounts();
  const sources = useLinearSources(project);
  const discovery = useLinearDiscovery(project);
  const del = useDeleteLinearSource(project);
  const navigate = useNavigate();

  if (home === "project") return <Navigate to={linearTab(project)} replace />;

  const binding = sources.data?.find((s) => s.workflow === workflow);
  const title = binding
    ? describeLinearWhen(binding, discovery.data?.teams)
    : workflow;

  return (
    <TriggerShell title={title}>
      {loading || sources.isLoading ? (
        <p className="text-xs text-muted-foreground">Loading…</p>
      ) : !usable ? (
        <NoLinearAccount />
      ) : !binding ? (
        <NotFound what={`a Linear trigger for ${workflow} in ${project}`} />
      ) : (
        <>
          <TriggerHeader
            kind="linear"
            title={title}
            workflow={workflow}
            project={project}
            state={fromBinding(binding).state}
          />
          <AccountSelect project={project} accounts={accounts} />
          {discovery.isError ? (
            <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive">
              {discovery.error.message}
            </div>
          ) : !discovery.data ? (
            <p className="text-xs text-muted-foreground">Loading…</p>
          ) : (
            <BindingForm
              key={binding.updated_at}
              project={project}
              workflow={workflow}
              fixedWorkflow
              source={binding}
              teams={discovery.data.teams}
              onDone={() => navigate(LIST)}
            />
          )}
          <WiringCheck project={project} />
          <DangerZone
            busy={del.isPending}
            error={del.error?.message}
            label="Delete trigger"
            note="It goes to Recently deleted on the Projects page for 14 days."
            onDelete={() =>
              del.mutate(workflow, {
                onSuccess: (res) =>
                  navigate(LIST, {
                    state: {
                      deleted: {
                        label: `the ${workflow} trigger in ${project}`,
                        trashId: res.trash_id,
                      },
                    } satisfies DeletedState,
                  }),
              })
            }
          />
        </>
      )}
    </TriggerShell>
  );
}

// ── Schedules ──────────────────────────────────────────────────────────────

function NewScheduleTrigger() {
  const [params] = useSearchParams();
  const admin = isAdminOf(useAuthStatus().data);
  const navigate = useNavigate();
  return (
    <TriggerShell title="New schedule">
      <TriggerHeader kind="schedule" title="New schedule" />
      {admin ? (
        <ScheduleForm
          schedule={null}
          initialProject={params.get("project") ?? undefined}
          readOnly={false}
          onSaved={() => navigate(LIST)}
          onCancel={() => navigate(LIST)}
        />
      ) : (
        <p className="text-sm text-muted-foreground">
          Only administrators create schedules.
        </p>
      )}
    </TriggerShell>
  );
}

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
            workflow={schedule.workflow}
            project={schedule.project}
            state={fromSchedule(schedule).state}
          />
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[12px] text-muted-foreground">
            <span>{describeScheduleWhen(schedule)}</span>
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
          {!admin && (
            <p className="text-[12px] text-muted-foreground">
              Only administrators change schedules.
            </p>
          )}
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
                remove.mutate(schedule.id, {
                  onSuccess: () =>
                    navigate(LIST, {
                      state: {
                        deleted: {
                          label: `the schedule “${schedule.name}”`,
                          trashId: null,
                        },
                      } satisfies DeletedState,
                    }),
                })
              }
            />
          )}
        </>
      )}
    </TriggerShell>
  );
}

// ── Shared ─────────────────────────────────────────────────────────────────

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
  /** Ask first: for a delete that can't be undone. */
  confirmText?: string;
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
            if (!confirmText || window.confirm(confirmText)) onDelete();
          }}
        >
          <IconTrash /> {busy ? "Deleting…" : "Delete"}
        </Button>
        {error && <span className="text-xs text-destructive">{error}</span>}
      </div>
    </section>
  );
}
