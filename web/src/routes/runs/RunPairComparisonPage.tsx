import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { AppShell } from "@/components/AppShell";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { TaskOverview } from "@/components/runflow/TaskOverview";
import {
  elapsedMs,
  formatDuration,
  formatTokens,
  sumUsage,
  totalTokens,
} from "@/components/runflow/format";
import { formatCost, usageCost } from "@/lib/cost";
import {
  nodesFromDetail,
  useJudgePair,
  useRunPair,
  useWorkflowModels,
} from "@/lib/runs";
import type {
  AbJudge,
  ModelRef,
  NodeView,
  RunDetail,
  RunStatus,
} from "@/types/run";

const STATUS_VARIANT: Record<
  RunStatus,
  "running" | "success" | "failed" | "skipped"
> = {
  running: "running",
  completed: "success",
  failed: "failed",
  cancelled: "failed",
};

interface ArmStats {
  detail: RunDetail;
  nodes: NodeView[];
  label: string;
  arm: string;
  tokens: number;
  cost: number;
  timeMs: number | null;
}

function armStats(detail: RunDetail, now: number): ArmStats {
  const nodes = nodesFromDetail(detail);
  const usage = sumUsage(nodes.map((n) => n.usage));
  const cost = nodes.reduce(
    (acc, n) => acc + usageCost(n.model ?? n.provider, n.usage),
    0,
  );
  return {
    detail,
    nodes,
    label: detail.ab_label ?? detail.ab_arm ?? detail.id,
    arm: (detail.ab_arm ?? "?").toUpperCase(),
    tokens: totalTokens(usage),
    cost,
    timeMs: elapsedMs(detail.started_at, detail.ended_at, now),
  };
}

/** B−A delta, coloured by whether lower is better (cost/tokens/time all are). */
function Delta({
  a,
  b,
  format,
}: {
  a: number | null;
  b: number | null;
  format: (n: number) => string;
}) {
  if (a == null || b == null || a === 0) return <>—</>;
  const diff = b - a;
  if (diff === 0) return <span className="text-muted-foreground">±0</span>;
  const pct = Math.round((diff / a) * 100);
  const better = diff < 0; // B is cheaper / faster / fewer tokens
  return (
    <span
      style={{
        color: better ? "var(--status-success)" : "var(--status-failed)",
      }}
    >
      {diff > 0 ? "+" : "−"}
      {format(Math.abs(diff))} ({pct > 0 ? "+" : ""}
      {pct}%)
    </span>
  );
}

export function RunPairComparisonPage() {
  const { pairId = null } = useParams();
  const now = Date.now();
  // Live-poll while either arm may still be running.
  const pair = useRunPair(pairId, true);
  const runs = pair.data?.runs ?? [];
  const arms = runs.map((r) => armStats(r, now));
  const [a, b] = arms;
  const title =
    runs[0]?.title ?? runs[0]?.workflow_name ?? pairId ?? "A/B pair";

  return (
    <AppShell title={`A/B · ${title}`}>
      <div className="flex flex-col gap-6 p-6">
        <div>
          <Link
            to="/runs"
            className="text-xs text-muted-foreground hover:text-foreground"
          >
            ← Runs
          </Link>
          <h1 className="mt-1 text-lg font-semibold">A/B comparison</h1>
          <p className="font-mono text-[11px] text-muted-foreground">
            {pairId}
          </p>
        </div>

        {pair.isLoading && (
          <p className="text-sm text-muted-foreground">Loading pair…</p>
        )}
        {pair.isError && (
          <p className="text-sm text-status-failed">
            {pair.error?.message ?? "Failed to load pair."}
          </p>
        )}
        {pair.data && arms.length < 2 && (
          <p className="text-sm text-muted-foreground">
            Only one arm found for this pair so far — the other may still be
            starting.
          </p>
        )}

        {a && b && (
          <div className="overflow-hidden border border-border">
            <table className="w-full text-[13px]">
              <thead className="bg-secondary/50 text-muted-foreground">
                <tr>
                  <Th className="text-left">Metric</Th>
                  <Th className="text-left">Arm A</Th>
                  <Th className="text-left">Arm B</Th>
                  <Th className="text-right">Δ (B vs A)</Th>
                </tr>
              </thead>
              <tbody>
                <Row label="Model">
                  <Td className="font-medium">{a.label}</Td>
                  <Td className="font-medium">{b.label}</Td>
                  <Td className="text-right text-muted-foreground">—</Td>
                </Row>
                <Row label="Status">
                  <Td>
                    <Badge
                      variant={STATUS_VARIANT[a.detail.status] ?? "default"}
                    >
                      {a.detail.status}
                    </Badge>
                  </Td>
                  <Td>
                    <Badge
                      variant={STATUS_VARIANT[b.detail.status] ?? "default"}
                    >
                      {b.detail.status}
                    </Badge>
                  </Td>
                  <Td className="text-right text-muted-foreground">—</Td>
                </Row>
                <Row label="Run time">
                  <Td className="tabular-nums">{formatDuration(a.timeMs)}</Td>
                  <Td className="tabular-nums">{formatDuration(b.timeMs)}</Td>
                  <Td className="text-right tabular-nums">
                    <Delta
                      a={a.timeMs}
                      b={b.timeMs}
                      format={(n) => formatDuration(n)}
                    />
                  </Td>
                </Row>
                <Row label="Tokens">
                  <Td className="tabular-nums">{formatTokens(a.tokens)}</Td>
                  <Td className="tabular-nums">{formatTokens(b.tokens)}</Td>
                  <Td className="text-right tabular-nums">
                    <Delta
                      a={a.tokens}
                      b={b.tokens}
                      format={(n) => formatTokens(n)}
                    />
                  </Td>
                </Row>
                <Row label="Notional cost">
                  <Td className="font-medium tabular-nums">
                    {formatCost(a.cost)}
                  </Td>
                  <Td className="font-medium tabular-nums">
                    {formatCost(b.cost)}
                  </Td>
                  <Td className="text-right tabular-nums">
                    <Delta
                      a={a.cost}
                      b={b.cost}
                      format={(n) => formatCost(n)}
                    />
                  </Td>
                </Row>
              </tbody>
            </table>
          </div>
        )}

        {a && b && pairId && (
          <JudgePanel
            pairId={pairId}
            project={a.detail.project}
            judge={pair.data?.judge ?? null}
            labelA={a.label}
            labelB={b.label}
          />
        )}

        {arms.map((arm) => (
          <section key={arm.detail.id} className="flex flex-col gap-2">
            <div className="flex items-center gap-2">
              <Badge variant="secondary">Arm {arm.arm}</Badge>
              <span className="text-sm font-medium">{arm.label}</span>
              <Link
                to={`/runs/${arm.detail.id}`}
                className="font-mono text-[11px] text-muted-foreground hover:text-foreground"
              >
                {arm.detail.id}
              </Link>
            </div>
            <TaskOverview nodes={arm.nodes} />
          </section>
        ))}
      </div>
    </AppShell>
  );
}

function Th({
  children,
  className = "",
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return <th className={`px-3 py-2 font-medium ${className}`}>{children}</th>;
}

function Td({
  children,
  className = "",
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return <td className={`px-3 py-2 ${className}`}>{children}</td>;
}

function Row({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <tr className="border-t border-border">
      <Td className="font-medium text-muted-foreground">{label}</Td>
      {children}
    </tr>
  );
}

/**
 * Quality judgement: a pairwise LLM verdict over both arms. Shows the verdict
 * once judged, a progress note while the judge run is in flight, or a judge-model
 * picker + trigger button otherwise. The picker prefills from the `judge-ab`
 * workflow's own default model (so there's no hardcoded id) and is overridable.
 */
function JudgePanel({
  pairId,
  project,
  judge,
  labelA,
  labelB,
}: {
  pairId: string;
  project: string | null;
  judge: AbJudge | null;
  labelA: string;
  labelB: string;
}) {
  const judgeModels = useWorkflowModels("judge-ab", project);
  const defaultModel = judgeModels.data?.[0] ?? null;
  const [override, setOverride] = useState<ModelRef | null>(null);
  const chosen = override ?? defaultModel;
  const run = useJudgePair(pairId);

  const verdict = judge?.verdict ?? null;
  const judging = !!judge && !verdict && judge.status !== "failed";

  return (
    <section className="border border-border">
      <header className="flex items-center justify-between border-b border-border bg-secondary/40 px-3 py-2">
        <span className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          Quality judgement
        </span>
        {judge?.run_id && (
          <Link
            to={`/runs/${judge.run_id}`}
            className="font-mono text-[11px] text-muted-foreground hover:text-foreground"
          >
            {judge.run_id}
          </Link>
        )}
      </header>

      <div className="p-3">
        {verdict ? (
          <div className="flex flex-col gap-2">
            <div className="flex items-center gap-2 text-sm">
              <span className="font-semibold">
                {verdict.winner === "tie"
                  ? "Tie"
                  : `Arm ${verdict.winner.toUpperCase()} wins`}
              </span>
              <span className="text-muted-foreground">
                — {labelA}{" "}
                <span className="tabular-nums">{verdict.score_a}</span> vs{" "}
                {labelB} <span className="tabular-nums">{verdict.score_b}</span>
              </span>
            </div>
            <p className="text-[13px] text-muted-foreground">
              {verdict.reasoning}
            </p>
            <button
              type="button"
              onClick={() => run.mutate({ judge_model: chosen ?? undefined })}
              disabled={run.isPending}
              className="self-start text-[11px] text-muted-foreground underline hover:text-foreground"
            >
              {run.isPending ? "Re-judging…" : "Re-judge"}
            </button>
          </div>
        ) : judging ? (
          <p className="text-sm text-muted-foreground">
            Judging in progress… the verdict appears here when the judge run
            finishes.
          </p>
        ) : (
          <div className="flex flex-col gap-2">
            <span className="text-[12px] text-muted-foreground">
              Score which arm did the better job. Judge model:
            </span>
            <div className="flex flex-wrap items-center gap-2">
              <input
                value={chosen?.provider ?? ""}
                onChange={(e) =>
                  setOverride({
                    provider: e.target.value,
                    model: chosen?.model ?? "",
                  })
                }
                placeholder="provider"
                className="h-8 w-28 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
              />
              <input
                value={chosen?.model ?? ""}
                onChange={(e) =>
                  setOverride({
                    provider: chosen?.provider ?? "",
                    model: e.target.value,
                  })
                }
                placeholder="model"
                className="h-8 flex-1 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
              />
              <Button
                type="button"
                onClick={() => run.mutate({ judge_model: chosen ?? undefined })}
                disabled={run.isPending || !chosen}
              >
                {run.isPending ? "Starting…" : "Judge quality"}
              </Button>
            </div>
            {run.isError && (
              <p className="text-xs text-destructive">{run.error.message}</p>
            )}
          </div>
        )}
      </div>
    </section>
  );
}
