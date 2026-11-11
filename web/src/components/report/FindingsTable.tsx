/**
 * The `layout: table` view of a report's findings, for reports with hundreds
 * of them (a test audit): a tile per category with its count and time, tabs to
 * pick a category, a search box, and one sortable row per finding. The detail
 * shows on one line and opens on click. Rows come in pages, so a report with
 * thousands of findings doesn't mount them all.
 */
import { type ReactNode, useMemo, useState } from "react";
import { ArrowDown, ArrowUp, Search } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupInput,
} from "@/components/ui/input-group";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";
import { findingKey, type WorkflowFinding } from "@/lib/report";
import {
  categoryOf,
  categoryTotals,
  columnsOf,
  defaultSort,
  formatMs,
  type Sort,
  type SortKey,
  visibleRows,
} from "@/lib/report-table";

const PAGE = 100;
const ALL = "__all__";

const SEVERITY_VARIANT: Record<string, "failed" | "running" | "secondary"> = {
  critical: "failed",
  high: "failed",
  medium: "running",
  low: "secondary",
  info: "secondary",
};

export function FindingsTable({
  findings,
  actionsFor,
  dimmed,
}: {
  findings: WorkflowFinding[];
  /** The buttons for one row; absent → no actions column. */
  actionsFor?: (f: WorkflowFinding) => ReactNode;
  /** Rows to show faded (an ignored finding). */
  dimmed?: (f: WorkflowFinding) => boolean;
}) {
  const totals = useMemo(() => categoryTotals(findings), [findings]);
  const cols = useMemo(() => columnsOf(findings), [findings]);
  // Opens on the most severe category: what needs a decision first.
  const [tab, setTab] = useState<string>(
    totals.length > 1 ? totals[0].category : ALL,
  );
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<Sort>(() => defaultSort(findings));
  const [limit, setLimit] = useState(PAGE);
  const [open, setOpen] = useState<Set<string>>(new Set());

  const category = tab === ALL ? null : tab;
  const rows = useMemo(
    () => visibleRows(findings, category, query, sort),
    [findings, category, query, sort],
  );
  const shown = rows.slice(0, limit);

  const sortBy = (key: SortKey) => {
    setSort((s) =>
      s.key === key
        ? { key, dir: s.dir === "asc" ? "desc" : "asc" }
        : { key, dir: key === "duration" ? "desc" : "asc" },
    );
    setLimit(PAGE);
  };
  const toggle = (key: string) =>
    setOpen((o) => {
      const next = new Set(o);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  return (
    <div className="flex flex-col gap-4">
      <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
        <Tile label="All" count={findings.length} durationMs={sumMs(totals)} />
        {totals.map((t) => (
          <Tile
            key={t.category}
            label={t.category}
            count={t.count}
            durationMs={t.durationMs}
          />
        ))}
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <Tabs
          value={tab}
          onValueChange={(v) => {
            setTab(String(v));
            setLimit(PAGE);
          }}
        >
          <TabsList>
            {totals.map((t) => (
              <TabsTrigger key={t.category} value={t.category}>
                {t.category}
                <span className="text-muted-foreground tabular-nums">
                  {t.count}
                </span>
              </TabsTrigger>
            ))}
            <TabsTrigger value={ALL}>
              All
              <span className="text-muted-foreground tabular-nums">
                {findings.length}
              </span>
            </TabsTrigger>
          </TabsList>
        </Tabs>
        <InputGroup className="ml-auto w-full sm:w-64">
          <InputGroupAddon>
            <Search />
          </InputGroupAddon>
          <InputGroupInput
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setLimit(PAGE);
            }}
            placeholder="Search title, location, reason"
            aria-label="Search findings"
          />
        </InputGroup>
      </div>

      <div className="rounded-md border border-border">
        <Table>
          <TableHeader>
            <TableRow>
              <SortHead label="Finding" k="title" sort={sort} onSort={sortBy} />
              {cols.location && (
                <SortHead
                  label="Location"
                  k="location"
                  sort={sort}
                  onSort={sortBy}
                />
              )}
              {cols.score && (
                <SortHead
                  label="Score"
                  k="score"
                  sort={sort}
                  onSort={sortBy}
                  numeric
                />
              )}
              {cols.duration && (
                <SortHead
                  label="Time"
                  k="duration"
                  sort={sort}
                  onSort={sortBy}
                  numeric
                />
              )}
              {category === null && <TableHead>Category</TableHead>}
              <TableHead className="w-full">Reason</TableHead>
              {actionsFor && <TableHead className="text-right" />}
            </TableRow>
          </TableHeader>
          <TableBody>
            {shown.length === 0 ? (
              <TableRow>
                <TableCell
                  colSpan={8}
                  className="py-6 text-center text-muted-foreground"
                >
                  {query ? "Nothing matches the search." : "No findings."}
                </TableCell>
              </TableRow>
            ) : (
              shown.map((f) => {
                const key = findingKey(f);
                const expanded = open.has(key);
                return (
                  <TableRow
                    key={key}
                    data-state={expanded ? "selected" : undefined}
                    className={cn(
                      "cursor-pointer align-top",
                      dimmed?.(f) && "opacity-50",
                    )}
                    onClick={() => toggle(key)}
                  >
                    <TableCell
                      className={cn(
                        "max-w-72 font-medium",
                        expanded ? "whitespace-normal" : "truncate",
                      )}
                      title={f.title ?? f.summary}
                    >
                      {f.title ?? f.summary ?? "(untitled)"}
                    </TableCell>
                    {cols.location && (
                      <TableCell
                        className={cn(
                          "max-w-56 font-mono text-[11px] text-muted-foreground",
                          expanded ? "break-all whitespace-normal" : "truncate",
                        )}
                        title={f.location}
                      >
                        {f.location}
                      </TableCell>
                    )}
                    {cols.score && (
                      <TableCell className="text-right tabular-nums">
                        {typeof f.score === "number" ? f.score.toFixed(2) : ""}
                      </TableCell>
                    )}
                    {cols.duration && (
                      <TableCell className="text-right whitespace-nowrap tabular-nums">
                        {typeof f.duration_ms === "number"
                          ? formatMs(f.duration_ms)
                          : ""}
                      </TableCell>
                    )}
                    {category === null && (
                      <TableCell>
                        <Badge
                          variant={
                            SEVERITY_VARIANT[f.severity ?? ""] ?? "secondary"
                          }
                        >
                          {categoryOf(f)}
                        </Badge>
                      </TableCell>
                    )}
                    <TableCell
                      className={cn(
                        "max-w-0 text-muted-foreground",
                        expanded ? "whitespace-pre-wrap" : "truncate",
                      )}
                      title={expanded ? undefined : f.detail}
                    >
                      {f.detail}
                      {expanded && f.fix && (
                        <span className="mt-1 block text-foreground">
                          Fix: {f.fix}
                        </span>
                      )}
                    </TableCell>
                    {actionsFor && (
                      <TableCell
                        className="text-right whitespace-nowrap"
                        // A button in the row must not also open it.
                        onClick={(e) => e.stopPropagation()}
                      >
                        {actionsFor(f)}
                      </TableCell>
                    )}
                  </TableRow>
                );
              })
            )}
          </TableBody>
        </Table>
      </div>

      <div className="flex items-center gap-3 text-xs text-muted-foreground">
        <span>
          Showing {shown.length} of {rows.length}
        </span>
        {rows.length > shown.length && (
          <Button
            variant="outline"
            size="sm"
            onClick={() => setLimit((l) => l + PAGE)}
          >
            Show {Math.min(PAGE, rows.length - shown.length)} more
          </Button>
        )}
      </div>
    </div>
  );
}

function sumMs(totals: { durationMs: number | null }[]): number | null {
  const known = totals.filter((t) => t.durationMs !== null);
  return known.length
    ? known.reduce((a, t) => a + (t.durationMs ?? 0), 0)
    : null;
}

function Tile({
  label,
  count,
  durationMs,
}: {
  label: string;
  count: number;
  durationMs: number | null;
}) {
  return (
    <Card size="sm">
      <CardContent className="flex flex-col gap-0.5">
        <span className="text-xs text-muted-foreground capitalize">
          {label}
        </span>
        <span className="text-2xl font-semibold tabular-nums">{count}</span>
        {durationMs !== null && (
          <span className="text-xs text-muted-foreground tabular-nums">
            {formatMs(durationMs)}
          </span>
        )}
      </CardContent>
    </Card>
  );
}

function SortHead({
  label,
  k,
  sort,
  onSort,
  numeric = false,
}: {
  label: string;
  k: SortKey;
  sort: Sort;
  onSort: (k: SortKey) => void;
  numeric?: boolean;
}) {
  const active = sort.key === k;
  return (
    <TableHead
      className={numeric ? "text-right" : undefined}
      aria-sort={
        active ? (sort.dir === "asc" ? "ascending" : "descending") : "none"
      }
    >
      <Button
        variant="ghost"
        size="xs"
        className={cn("-mx-2", active && "text-foreground")}
        onClick={() => onSort(k)}
      >
        {label}
        {active &&
          (sort.dir === "asc" ? (
            <ArrowUp data-icon="inline-end" />
          ) : (
            <ArrowDown data-icon="inline-end" />
          ))}
      </Button>
    </TableHead>
  );
}
