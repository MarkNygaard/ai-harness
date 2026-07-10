import { X } from "lucide-react";
import { NAV_ICON_KEYS } from "@/components/AppSidebar";
import type {
  ReportAction,
  ReportStatus,
  WorkflowNav,
  WorkflowReport,
  WorkflowUi,
} from "@/types/authoring";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Input } from "@/components/ui/input";

/** Sentinel for a nullable select — Base UI Select needs a concrete item value. */
const AUTO_SENTINEL = "__auto__";
const REPORT_ACTIONS: ReportAction[] = ["build", "issue", "ignore"];
const REPORT_STATUSES: ReportStatus[] = ["none", "check", "pass_fail"];

/**
 * Right drawer: view and edit a workflow's declarative **UI Block** — its
 * left-nav entry (`ui.nav`) and report/findings tab (`ui.report`). Without this
 * the block was invisible in the builder and silently dropped on save; now the
 * canvas is the source of truth for it too.
 */
export function WorkflowSettingsDrawer({
  ui,
  nodeIds,
  onChange,
  onClose,
}: {
  ui: WorkflowUi | null | undefined;
  nodeIds: string[];
  onChange: (next: WorkflowUi | undefined) => void;
  onClose: () => void;
}) {
  const nav = ui?.nav ?? null;
  const report = ui?.report ?? null;

  // Emit `undefined` when neither surface is declared, so an empty UI block
  // never gets written to YAML.
  const emit = (
    nextNav: WorkflowNav | null,
    nextReport: WorkflowReport | null,
  ) =>
    onChange(
      nextNav || nextReport ? { nav: nextNav, report: nextReport } : undefined,
    );
  const setNav = (next: WorkflowNav | null) => emit(next, report);
  const setReport = (next: WorkflowReport | null) => emit(nav, next);

  const toggleAction = (action: ReportAction, on: boolean) => {
    if (!report) return;
    const current = report.actions ?? [];
    const actions = on
      ? [...current, action]
      : current.filter((a) => a !== action);
    setReport({ ...report, actions: actions.length ? actions : undefined });
  };

  return (
    <div className="flex w-1/3 min-w-[20rem] flex-none flex-col border-l border-border bg-card">
      <div className="flex items-center justify-between border-b border-border px-4 py-3">
        <span className="text-sm font-semibold">Workflow UI</span>
        <button
          type="button"
          onClick={onClose}
          className="rounded p-1 hover:bg-secondary"
        >
          <X className="h-4 w-4" />
        </button>
      </div>

      <div className="flex flex-col gap-3 overflow-auto p-4 text-[13px]">
        <p className="text-[12px] text-muted-foreground">
          Optional surfaces this workflow adds once it has a run: a left-nav
          entry and a report / findings tab on each run.
        </p>

        {/* Left-nav entry */}
        <Toggle
          label="Show a left-nav entry"
          checked={!!nav}
          onChange={(on) => setNav(on ? { label: "", icon: null } : null)}
        />
        {nav && (
          <div className="flex flex-col gap-3 border-l-2 border-border pl-3">
            <Field label="Nav label">
              <Input
                value={nav.label}
                placeholder="e.g. Test Scenarios"
                onChange={(e) => setNav({ ...nav, label: e.target.value })}
              />
            </Field>
            <SelectField
              label="Icon"
              value={nav.icon ?? AUTO_SENTINEL}
              onValueChange={(v) =>
                setNav({ ...nav, icon: v === AUTO_SENTINEL ? null : v })
              }
            >
              <SelectItem value={AUTO_SENTINEL}>(default)</SelectItem>
              {NAV_ICON_KEYS.map((k) => (
                <SelectItem key={k} value={k}>
                  {k}
                </SelectItem>
              ))}
            </SelectField>
          </div>
        )}

        {/* Report / findings tab */}
        <div className="mt-1 border-t border-border pt-3" />
        <Toggle
          label="Add a report / findings tab"
          checked={!!report}
          onChange={(on) =>
            setReport(
              on ? { label: "", verdict_node: null, scored: false } : null,
            )
          }
        />
        {report && (
          <div className="flex flex-col gap-3 border-l-2 border-border pl-3">
            <Field label="Tab label">
              <Input
                value={report.label}
                placeholder="e.g. Scenarios"
                onChange={(e) =>
                  setReport({ ...report, label: e.target.value })
                }
              />
            </Field>
            <SelectField
              label="Verdict node"
              value={report.verdict_node ?? AUTO_SENTINEL}
              onValueChange={(v) =>
                setReport({
                  ...report,
                  verdict_node: v === AUTO_SENTINEL ? null : v,
                })
              }
            >
              <SelectItem value={AUTO_SENTINEL}>(auto-scan nodes)</SelectItem>
              {nodeIds.map((id) => (
                <SelectItem key={id} value={id}>
                  {id}
                </SelectItem>
              ))}
            </SelectField>
            <SelectField
              label="Per-item status"
              value={report.status ?? "none"}
              onValueChange={(v) =>
                setReport({ ...report, status: v as ReportStatus })
              }
            >
              {REPORT_STATUSES.map((s) => (
                <SelectItem key={s} value={s}>
                  {s}
                </SelectItem>
              ))}
            </SelectField>
            <Toggle
              label="Scored (score gauge + history)"
              checked={report.scored}
              onChange={(on) => setReport({ ...report, scored: on })}
            />
            <div className="flex flex-col gap-1">
              <span className="text-[11px] font-medium text-muted-foreground">
                Per-finding actions
              </span>
              {REPORT_ACTIONS.map((a) => (
                <Toggle
                  key={a}
                  label={a}
                  checked={(report.actions ?? []).includes(a)}
                  onChange={(on) => toggleAction(a, on)}
                />
              ))}
            </div>
          </div>
        )}
      </div>
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

function Toggle({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: (on: boolean) => void;
}) {
  return (
    <label className="flex items-center gap-2 text-[12px] text-muted-foreground">
      <input
        type="checkbox"
        className="size-3.5"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
      {label}
    </label>
  );
}

/** A labelled Select mirroring PropertiesDrawer's — the trigger is a button, so
 *  it is not wrapped in a `<label>`. */
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
          <SelectValue />
        </SelectTrigger>
        <SelectContent>{children}</SelectContent>
      </Select>
    </div>
  );
}
