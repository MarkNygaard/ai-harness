import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import {
  IconBinaryTree2,
  IconLayoutGrid,
  IconLayoutList,
  IconPlus,
} from "@tabler/icons-react";
import { AppShell } from "@/components/AppShell";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useWorkflowList } from "@/lib/authoring";
import { titleFromSlug } from "@/lib/workflow-name";
import type { WorkflowSummary } from "@/types/authoring";

type View = "grid" | "list";

/** Personal, per-browser layout choice for this page. */
const VIEW_KEY = "harness.workflows.view";

function loadView(): View {
  try {
    return localStorage.getItem(VIEW_KEY) === "list" ? "list" : "grid";
  } catch {
    return "grid";
  }
}

/** Browse bundled + project workflows; click to edit, or create a new one. */
export function WorkflowsList() {
  const workflows = useWorkflowList();
  const [view, setView] = useState<View>(loadView);

  useEffect(() => {
    try {
      localStorage.setItem(VIEW_KEY, view);
    } catch {
      /* private mode / storage disabled — the choice just won't persist */
    }
  }, [view]);

  // The API already resolves shadowing (a project workflow hides the bundled
  // default of the same name), so a name appears in exactly one group.
  const all = workflows.data ?? [];
  const yours = all.filter((wf) => wf.source === "project");
  const templates = all.filter((wf) => wf.source === "bundled");

  return (
    <AppShell
      title="Workflows"
      viewActions={
        <>
          <ViewToggle view={view} onChange={setView} />
          <Button size="sm" render={<Link to="/editor/new" />}>
            <IconPlus className="size-4" />
            New workflow
          </Button>
        </>
      }
    >
      <div className="mx-auto flex max-w-5xl flex-col gap-6 p-6">
        <div className="sm:pr-64">
          <h1 className="flex items-center gap-2 text-lg font-semibold">
            <IconBinaryTree2 className="size-5 text-accent-orange" />
            Workflows
          </h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Edit a workflow in the visual builder. Bundled defaults are
            read-only templates — saving one creates an editable project copy
            that shadows it.
          </p>
        </div>

        {workflows.isLoading && (
          <p className="text-sm text-muted-foreground">Loading…</p>
        )}
        {workflows.isError && (
          <p className="text-sm text-destructive">
            Failed to load workflows: {workflows.error.message}
          </p>
        )}
        {!workflows.isLoading && all.length === 0 && (
          <p className="text-sm text-muted-foreground">No workflows yet.</p>
        )}

        <Section title="Yours" count={yours.length} view={view}>
          {yours.map((wf) => (
            <WorkflowCard key={wf.name} wf={wf} view={view} />
          ))}
        </Section>
        {yours.length === 0 && all.length > 0 && (
          <p className="-mt-4 text-xs text-muted-foreground">
            None yet — saving a template below creates an editable copy here.
          </p>
        )}

        <Section
          title="Templates"
          count={templates.length}
          note="read-only"
          view={view}
        >
          {templates.map((wf) => (
            <WorkflowCard key={wf.name} wf={wf} view={view} />
          ))}
        </Section>
      </div>
    </AppShell>
  );
}

/** Grid/list switch. Icon-only — the two layouts are self-evident. */
function ViewToggle({
  view,
  onChange,
}: {
  view: View;
  onChange: (v: View) => void;
}) {
  const options: { value: View; label: string; Icon: typeof IconLayoutGrid }[] =
    [
      { value: "grid", label: "Grid view", Icon: IconLayoutGrid },
      { value: "list", label: "List view", Icon: IconLayoutList },
    ];
  return (
    <div
      className="flex items-center rounded-md border border-border p-0.5"
      role="group"
      aria-label="Layout"
    >
      {options.map(({ value, label, Icon }) => (
        <Button
          key={value}
          variant={view === value ? "secondary" : "ghost"}
          size="icon-sm"
          title={label}
          aria-label={label}
          aria-pressed={view === value}
          onClick={() => onChange(value)}
        >
          <Icon className="size-4" />
        </Button>
      ))}
    </div>
  );
}

/** A titled group. Renders nothing when empty so the page stays quiet. */
function Section({
  title,
  count,
  note,
  view,
  children,
}: {
  title: string;
  count: number;
  note?: string;
  view: View;
  children: React.ReactNode;
}) {
  if (count === 0) return null;
  return (
    <section className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          {title}
        </h2>
        {note && (
          <span className="text-[11px] text-muted-foreground">{note}</span>
        )}
        <span className="ml-auto text-[11px] tabular-nums text-muted-foreground">
          {count}
        </span>
      </div>
      <div
        className={
          view === "grid"
            ? "grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3"
            : "flex flex-col gap-2"
        }
      >
        {children}
      </div>
    </section>
  );
}

function WorkflowCard({ wf, view }: { wf: WorkflowSummary; view: View }) {
  const steps = `${wf.node_count} step${wf.node_count === 1 ? "" : "s"}`;
  // Readable heading, canonical slug kept alongside: the slug is what you type
  // in YAML, pass to MCP and bind Linear triggers to, so hiding it would cost
  // more than the prettier title gains.
  const title = titleFromSlug(wf.name);

  return (
    <Link
      to={`/editor/${encodeURIComponent(wf.name)}`}
      className="group block"
      title={wf.description ?? title}
    >
      <Card className="h-full transition-colors group-hover:border-accent-orange/50">
        {view === "grid" ? (
          // Portrait by construction: a 4:5 aspect ratio is taller than wide at
          // every breakpoint and makes every card in a row exactly the same
          // height, so no pixel height needs maintaining as the columns change.
          // Single-column (narrow screens) falls back to a short fixed height —
          // a full-width portrait card would be absurdly tall.
          <CardContent className="flex h-40 flex-col gap-2 py-3 sm:h-auto sm:aspect-4/5">
            <div className="flex items-start gap-2">
              <IconBinaryTree2 className="mt-0.5 size-4 shrink-0 text-accent-orange" />
              <div className="min-w-0">
                <span className="line-clamp-2 text-sm font-medium leading-snug">
                  {title}
                </span>
                <span className="block truncate font-mono text-[10px] text-muted-foreground">
                  {wf.name}
                </span>
              </div>
            </div>
            {/* Flows to the bottom of the card and ellipsises there. CSS can only
                clamp to a *line count*, never to a container height, so the count
                is tuned to the 4:5 box: at three columns the text area is ~320px
                and `text-xs` lines are 16px, so ~20 lines reaches the footer.
                Descriptions here run 7-20 paragraph-lines, so the clamp is
                virtually always what truncates — the dots land at the card edge. */}
            <p className="line-clamp-3 flex-1 overflow-hidden text-xs leading-4 text-muted-foreground sm:line-clamp-20">
              {wf.description ?? ""}
            </p>
            <div className="text-[11px] tabular-nums text-muted-foreground">
              {steps}
            </div>
          </CardContent>
        ) : (
          <CardContent className="flex items-center gap-3 py-3">
            <IconBinaryTree2 className="size-5 shrink-0 text-accent-orange" />
            <div className="min-w-0 flex-1">
              <div className="flex min-w-0 items-baseline gap-2">
                <span className="truncate text-sm font-medium">{title}</span>
                <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
                  {wf.name}
                </span>
              </div>
              {wf.description && (
                <p className="mt-0.5 line-clamp-2 text-xs text-muted-foreground">
                  {wf.description}
                </p>
              )}
            </div>
            <div className="shrink-0 text-right text-[11px] tabular-nums text-muted-foreground">
              {steps}
            </div>
          </CardContent>
        )}
      </Card>
    </Link>
  );
}
