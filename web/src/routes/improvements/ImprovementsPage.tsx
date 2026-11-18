/**
 * Improvement suggestions: what a workflow's own runs suggest changing, and
 * where each workflow stands towards its next review. A person decides here:
 * a repo change is applied by a run that opens a pull request, a workflow
 * change is made in the editor and marked applied, and either can be
 * dismissed so the next review doesn't raise it again.
 */
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { toast } from "sonner";
import { AppShell } from "@/components/AppShell";
import { Markdown } from "@/components/Markdown";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  actionsFor,
  editorLink,
  filterOptions,
  type ImprovementSuggestion,
  type SuggestionStatus,
  useApplySuggestion,
  useImprovements,
  useReviewNow,
  useSetSuggestionStatus,
  type WorkflowStanding,
} from "@/lib/improvements";

const ALL = "__all__";
const STATUSES: { value: SuggestionStatus; label: string }[] = [
  { value: "new", label: "Open" },
  { value: "applied", label: "Applied" },
  { value: "dismissed", label: "Dismissed" },
];

function isStatus(v: string | null): v is SuggestionStatus {
  return v === "new" || v === "applied" || v === "dismissed";
}

export function ImprovementsPage() {
  const [params, setParams] = useSearchParams();
  const rawStatus = params.get("status");
  const status: SuggestionStatus = isStatus(rawStatus) ? rawStatus : "new";
  const workflow = params.get("workflow") ?? "";
  const project = params.get("project") ?? "";

  // The options come from an unfiltered read, so picking one workflow
  // doesn't empty the list of the others.
  const everything = useImprovements();
  const shown = useImprovements({
    status,
    workflow: workflow || undefined,
    project: project || undefined,
  });
  const options = filterOptions(everything.data);

  const setParam = (key: string, value: string) => {
    const next = new URLSearchParams(params);
    if (value) next.set(key, value);
    else next.delete(key);
    setParams(next, { replace: true });
  };

  const standings = (everything.data?.workflows ?? []).filter(
    (w) =>
      (!workflow || w.workflow === workflow) &&
      (!project || w.project === project),
  );

  return (
    <AppShell title="Improvements">
      <div className="mx-auto flex max-w-5xl flex-col gap-6 p-6">
        <p className="max-w-3xl text-sm text-muted-foreground">
          After a set number of finished runs, the harness reviews a workflow's
          runs in a project and suggests changes: mostly to the project's repo
          (its <code className="font-mono text-xs">CLAUDE.md</code>, skills,
          lint rules), sometimes to the workflow. Nothing changes until you act
          on a suggestion. Dismissing one tells the next review not to raise it
          again.
        </p>

        <div className="flex flex-wrap items-center gap-3">
          <Tabs
            value={status}
            onValueChange={(v) =>
              setParam("status", v === "new" ? "" : String(v))
            }
          >
            <TabsList>
              {STATUSES.map((s) => (
                <TabsTrigger key={s.value} value={s.value}>
                  {s.label}
                </TabsTrigger>
              ))}
            </TabsList>
          </Tabs>
          <FilterSelect
            label="All workflows"
            value={workflow}
            options={options.workflows}
            onChange={(v) => setParam("workflow", v)}
          />
          <FilterSelect
            label="All projects"
            value={project}
            options={options.projects}
            onChange={(v) => setParam("project", v)}
          />
        </div>

        {everything.isError && (
          <p className="text-sm text-destructive">{everything.error.message}</p>
        )}

        {standings.length > 0 && <Reviews standings={standings} />}

        <section className="flex flex-col gap-3">
          <h2 className="text-sm font-semibold">Suggestions</h2>
          {shown.isLoading && (
            <p className="text-sm text-muted-foreground">Loading…</p>
          )}
          {shown.isError && (
            <p className="text-sm text-destructive">{shown.error.message}</p>
          )}
          {shown.data && shown.data.suggestions.length === 0 && (
            <p className="text-sm text-muted-foreground">
              {status === "new"
                ? "No open suggestions."
                : status === "applied"
                  ? "No applied suggestions."
                  : "No dismissed suggestions."}
            </p>
          )}
          {shown.data?.suggestions.map((s) => (
            <SuggestionCard key={s.id} s={s} />
          ))}
        </section>
      </div>
    </AppShell>
  );
}

function FilterSelect({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string;
  options: string[];
  onChange: (v: string) => void;
}) {
  return (
    <Select
      value={value || ALL}
      onValueChange={(v) => onChange(v == null || v === ALL ? "" : String(v))}
    >
      <SelectTrigger className="w-48">
        <SelectValue placeholder={label} />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value={ALL}>{label}</SelectItem>
        {options.map((o) => (
          <SelectItem key={o} value={o}>
            {o}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

/** Where each workflow stands towards its next review, and the last one. */
function Reviews({ standings }: { standings: WorkflowStanding[] }) {
  const reviewNow = useReviewNow();
  return (
    <section className="flex flex-col gap-3">
      <h2 className="text-sm font-semibold">Reviews</h2>
      <div className="rounded-md border border-border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Workflow</TableHead>
              <TableHead>Project</TableHead>
              <TableHead className="text-right">Next review</TableHead>
              <TableHead>Last review</TableHead>
              <TableHead />
            </TableRow>
          </TableHeader>
          <TableBody>
            {standings.map((w) => {
              const key = `${w.workflow}/${w.project}`;
              const busy =
                reviewNow.isPending &&
                reviewNow.variables?.workflow === w.workflow &&
                reviewNow.variables?.project === w.project;
              const r = w.review;
              return (
                <TableRow key={key}>
                  <TableCell className="font-medium">{w.workflow}</TableCell>
                  <TableCell>{w.project}</TableCell>
                  <TableCell
                    className="text-right tabular-nums"
                    title={`A review every ${w.every} finished runs`}
                  >
                    {w.runs_since_review} / {w.every} runs
                  </TableCell>
                  <TableCell className="max-w-96 whitespace-normal">
                    {r ? (
                      <div className="flex flex-col gap-0.5">
                        <span className="text-xs text-muted-foreground">
                          {r.status === "done" && r.finished_at
                            ? new Date(r.finished_at).toLocaleString()
                            : r.status}
                          {r.run_id && (
                            <>
                              {" · "}
                              <Link
                                to={`/runs/${encodeURIComponent(r.run_id)}`}
                                className="hover:underline"
                              >
                                run
                              </Link>
                            </>
                          )}
                        </span>
                        {r.error ? (
                          <span className="text-xs text-destructive">
                            {r.error}
                          </span>
                        ) : (
                          r.summary && (
                            <span
                              className="line-clamp-2 text-xs"
                              title={r.summary}
                            >
                              {r.summary}
                            </span>
                          )
                        )}
                      </div>
                    ) : (
                      <span className="text-xs text-muted-foreground">
                        None yet
                      </span>
                    )}
                  </TableCell>
                  <TableCell className="text-right">
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={busy}
                      onClick={() =>
                        reviewNow.mutate(
                          { workflow: w.workflow, project: w.project },
                          {
                            onSuccess: (res) =>
                              toast.success(
                                res.running
                                  ? `A review of ${w.workflow} is already running`
                                  : `Reviewing ${w.workflow}'s latest runs in ${w.project}`,
                              ),
                            onError: (e) => toast.error(e.message),
                          },
                        )
                      }
                    >
                      Review now
                    </Button>
                  </TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      </div>
    </section>
  );
}

function SuggestionCard({ s }: { s: ImprovementSuggestion }) {
  const navigate = useNavigate();
  const setStatus = useSetSuggestionStatus();
  const apply = useApplySuggestion();
  const busy = setStatus.isPending || apply.isPending;

  const status = (next: SuggestionStatus, done: string) =>
    setStatus.mutate(
      { id: s.id, status: next },
      {
        onSuccess: () => toast.success(done),
        onError: (e) => toast.error(e.message),
      },
    );

  return (
    <Card>
      <CardHeader>
        <CardTitle>{s.title}</CardTitle>
        <CardDescription className="flex flex-wrap items-center gap-1.5">
          <Badge variant="secondary">
            {s.target === "repo" ? "Repo change" : "Workflow change"}
          </Badge>
          <Badge variant="outline">{s.category}</Badge>
          <span>
            {s.workflow} · {s.project}
            {s.node && (
              <>
                {" · "}
                <code className="font-mono text-xs">{s.node}</code>
              </>
            )}
          </span>
        </CardDescription>
        <CardAction className="text-xs text-muted-foreground">
          {s.seen_count > 1
            ? `Raised by ${s.seen_count} reviews`
            : "Raised by 1 review"}
        </CardAction>
      </CardHeader>
      <CardContent className="flex flex-col gap-4 text-sm">
        <div className="flex flex-col gap-1">
          <h3 className="text-xs font-semibold text-muted-foreground">
            Problem
          </h3>
          <Markdown>{s.problem}</Markdown>
        </div>
        <div className="flex flex-col gap-1">
          <h3 className="text-xs font-semibold text-muted-foreground">
            Change
          </h3>
          <Markdown>{s.change}</Markdown>
        </div>
        {s.evidence.length > 0 && (
          <div className="flex flex-col gap-1">
            <h3 className="text-xs font-semibold text-muted-foreground">
              Evidence
            </h3>
            <ul className="flex flex-col gap-1.5 text-xs">
              {s.evidence.map((e, i) => (
                <li key={`${e.run_id}-${i}`}>
                  <Link
                    to={`/runs/${encodeURIComponent(e.run_id)}`}
                    className="font-mono hover:underline"
                  >
                    {e.run_id}
                  </Link>
                  {e.node && (
                    <span className="text-muted-foreground"> · {e.node}</span>
                  )}
                  <span className="text-muted-foreground"> — </span>
                  {e.note}
                </li>
              ))}
            </ul>
          </div>
        )}
      </CardContent>
      <CardFooter className="flex flex-wrap items-center gap-2">
        {actionsFor(s).map((a) => {
          switch (a) {
            case "apply":
              return (
                <Button
                  key={a}
                  size="sm"
                  disabled={busy}
                  title="Start a run that makes this change as a pull request"
                  onClick={() =>
                    apply.mutate(s.id, {
                      onSuccess: ({ run_id }) =>
                        toast.success("Started a run to make the change", {
                          action: {
                            label: "View run",
                            onClick: () =>
                              navigate(`/runs/${encodeURIComponent(run_id)}`),
                          },
                        }),
                      onError: (e) => toast.error(e.message),
                    })
                  }
                >
                  Apply as a pull request
                </Button>
              );
            case "open-editor":
              return (
                <Button
                  key={a}
                  size="sm"
                  variant="outline"
                  render={<Link to={editorLink(s)} />}
                >
                  Open in editor
                </Button>
              );
            case "mark-applied":
              return (
                <Button
                  key={a}
                  size="sm"
                  disabled={busy}
                  title="You made this change in the workflow"
                  onClick={() => status("applied", "Marked as applied")}
                >
                  Mark applied
                </Button>
              );
            case "dismiss":
              return (
                <Button
                  key={a}
                  size="sm"
                  variant="ghost"
                  disabled={busy}
                  title="The next review won't raise this again"
                  onClick={() => status("dismissed", "Dismissed")}
                >
                  Dismiss
                </Button>
              );
            case "reopen":
              return (
                <Button
                  key={a}
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() => status("new", "Reopened")}
                >
                  Reopen
                </Button>
              );
          }
        })}
        {s.status === "applied" && s.applied_run_id && (
          <Link
            to={`/runs/${encodeURIComponent(s.applied_run_id)}`}
            className="text-xs text-muted-foreground hover:underline"
          >
            The run that made it
          </Link>
        )}
        <span className="ml-auto text-xs text-muted-foreground">
          {s.status === "new"
            ? `Last raised ${new Date(s.last_seen_at).toLocaleDateString()}`
            : `${s.status === "applied" ? "Applied" : "Dismissed"} ${new Date(
                s.updated_at,
              ).toLocaleDateString()}`}
        </span>
      </CardFooter>
    </Card>
  );
}
