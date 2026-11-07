import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { IconPlus } from "@tabler/icons-react";
import { AppShell } from "@/components/AppShell";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ProjectLinearBindings } from "@/components/projects/ProjectLinear";
import { useAllLinearSources, useLinearConnections } from "@/lib/linear";
import { useProjects } from "@/lib/projects";

/**
 * Every Linear trigger in one place, grouped by project.
 *
 * The bindings themselves stay keyed by `(project, workflow)` — this is a
 * different way into the same rows, not a different model. Each group renders
 * the project tab's own editor rather than a second copy of the form: the
 * binding has eighteen fields and two places to maintain them is one too many.
 *
 * Project-first, unlike Schedules, because a binding's team and status options
 * come from *that project's* Linear connection. There is no team list to show
 * until a project is chosen.
 */
export function TriggersPage() {
  const sources = useAllLinearSources();
  const projects = useProjects();
  const connections = useLinearConnections();
  const accounts = connections.data ?? [];
  const [adding, setAdding] = useState<string | null>(null);

  // Projects that already have a binding, in the order the server returned
  // them (project, then workflow), so the page reads the same every visit.
  const withBindings = useMemo(() => {
    const seen: string[] = [];
    for (const s of sources.data ?? []) {
      if (!seen.includes(s.project)) seen.push(s.project);
    }
    return seen;
  }, [sources.data]);

  const unbound = (projects.data ?? [])
    .map((p) => p.name)
    .filter((n) => !withBindings.includes(n));

  const usable = accounts.some((c) => c.mode !== "none");

  return (
    <AppShell title="Triggers">
      <div className="flex flex-col gap-6 p-4">
        <p className="max-w-3xl text-sm text-muted-foreground">
          Which Linear teams start runs, and what they start. A trigger maps one
          team to one workflow in one project: the source status that fires it,
          the rest of the status map, and the base branch. Work begins when an
          issue is both <strong>delegated to the harness in Linear</strong> and
          sitting in that source status.
        </p>

        {connections.isLoading || sources.isLoading ? (
          <p className="text-xs text-muted-foreground">Loading…</p>
        ) : !usable ? (
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
        ) : (
          <>
            {sources.isError && (
              <p className="text-xs text-destructive">
                {sources.error.message}
              </p>
            )}

            {withBindings.length === 0 && adding === null && (
              <p className="text-sm text-muted-foreground">
                No triggers yet. Pick a project below to add the first one.
              </p>
            )}

            {withBindings.map((project) => (
              <section key={project} className="flex flex-col gap-2">
                <div className="flex items-center gap-2">
                  <h2 className="text-sm font-semibold">{project}</h2>
                  <Badge variant="outline" className="text-[10px]">
                    {
                      (sources.data ?? []).filter((s) => s.project === project)
                        .length
                    }{" "}
                    {(sources.data ?? []).filter((s) => s.project === project)
                      .length === 1
                      ? "trigger"
                      : "triggers"}
                  </Badge>
                </div>
                <ProjectLinearBindings project={project} accounts={accounts} />
              </section>
            ))}

            {/* A project with no binding has nothing for the loop above to
                render, so adding one needs its own way in. */}
            {adding !== null && (
              <section className="flex flex-col gap-2">
                <h2 className="text-sm font-semibold">{adding}</h2>
                <ProjectLinearBindings project={adding} accounts={accounts} />
              </section>
            )}

            {unbound.length > 0 && (
              <div className="flex items-center gap-2">
                <Select
                  value={adding ?? ""}
                  onValueChange={(v) => setAdding(v || null)}
                >
                  <SelectTrigger className="w-64">
                    <SelectValue placeholder="Add a trigger for…" />
                  </SelectTrigger>
                  <SelectContent>
                    {unbound.map((n) => (
                      <SelectItem key={n} value={n}>
                        {n}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                {adding !== null && (
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => setAdding(null)}
                  >
                    Done
                  </Button>
                )}
              </div>
            )}

            {unbound.length === 0 && withBindings.length > 0 && (
              <p className="text-[11px] text-muted-foreground">
                <IconPlus className="inline size-3" /> Every project has at
                least one trigger. Add more inside a project above.
              </p>
            )}
          </>
        )}
      </div>
    </AppShell>
  );
}
