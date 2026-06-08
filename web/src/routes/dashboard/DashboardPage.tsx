import { Link } from "react-router-dom";
import { AppShell } from "@/components/AppShell";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { useRunsSummary } from "@/lib/runs";
import { buildProjectSummaries, recentDayKeys } from "@/lib/dashboard";

export function DashboardPage() {
  const summary = useRunsSummary(14);
  const projects = buildProjectSummaries(summary.data ?? []);
  const now = new Date();
  const [today, yesterday, ...trailing] = recentDayKeys(now, 14);

  return (
    <AppShell title="Dashboard">
      <div className="mx-auto flex max-w-5xl flex-col gap-6 p-6">
        {summary.isLoading && (
          <p className="text-sm text-muted-foreground">Loading…</p>
        )}
        {summary.isError && (
          <p className="text-sm text-destructive">
            Failed to load summary: {summary.error.message}
          </p>
        )}
        {!summary.isLoading && !summary.isError && projects.length === 0 && (
          <p className="text-sm text-muted-foreground">
            No completed runs in the last 14 days.{" "}
            <Link to="/runs" className="underline">
              View all runs
            </Link>
          </p>
        )}
        <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
          {projects.map((ps) => {
            const runsLink = ps.isUnassigned
              ? "/runs?unassigned=true"
              : `/runs?project=${encodeURIComponent(ps.projectQuery ?? "")}`;
            return (
              <Card
                key={`${ps.isUnassigned ? "unassigned" : "project"}:${ps.project}`}
              >
                <CardHeader className="flex flex-row items-center justify-between pb-2">
                  <CardTitle className="text-sm font-medium">
                    <Link
                      to={runsLink}
                      className={
                        ps.isUnassigned
                          ? "text-muted-foreground hover:underline"
                          : "hover:underline"
                      }
                    >
                      {ps.project}
                    </Link>
                  </CardTitle>
                  <Badge variant="success">{ps.total}</Badge>
                </CardHeader>
                <CardContent>
                  <div className="flex flex-col gap-3">
                    <DayRow label="Today" day={today} byDay={ps.byDay} />
                    <DayRow
                      label="Yesterday"
                      day={yesterday}
                      byDay={ps.byDay}
                    />
                    <div className="flex gap-1">
                      {trailing.map((d) => {
                        const b = ps.byDay[d];
                        const failed = b?.failed ?? 0;
                        const hasFailed = failed > 0;
                        const completed = b?.completed ?? 0;
                        return (
                          <div
                            key={d}
                            className={`flex h-6 flex-1 items-center justify-center rounded text-[10px] font-medium ${
                              completed > 0
                                ? "bg-status-success/15 text-status-success"
                                : "bg-muted text-muted-foreground"
                            } ${hasFailed ? "ring-1 ring-status-failed" : ""}`}
                            title={`${d}: ${completed} completed${
                              hasFailed ? `, ${failed} failed` : ""
                            }`}
                          >
                            {completed > 0 ? completed : "·"}
                          </div>
                        );
                      })}
                    </div>
                  </div>
                </CardContent>
              </Card>
            );
          })}
        </div>
      </div>
    </AppShell>
  );
}

function DayRow({
  label,
  day,
  byDay,
}: {
  label: string;
  day: string;
  byDay: Record<string, { completed: number; failed: number }>;
}) {
  const b = byDay[day];
  const completed = b?.completed ?? 0;
  const failed = b?.failed ?? 0;
  return (
    <div className="flex items-center justify-between">
      <span className="text-xs text-muted-foreground">{label}</span>
      <div className="flex items-center gap-2">
        {completed > 0 && <Badge variant="success">{completed}</Badge>}
        {failed > 0 && <Badge variant="failed">{failed}</Badge>}
        {completed === 0 && failed === 0 && (
          <span className="text-xs text-muted-foreground">—</span>
        )}
      </div>
    </div>
  );
}
