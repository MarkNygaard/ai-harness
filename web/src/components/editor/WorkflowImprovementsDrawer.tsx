import { useEffect, useState } from "react";
import { X } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import type { ImprovementsConfig } from "@/types/authoring";

/** The bounds the server accepts for `improvements.every`. */
export const IMPROVEMENTS_MIN = 3;
export const IMPROVEMENTS_MAX = 100;
/** What switching it on starts at — the documented default. */
export const IMPROVEMENTS_DEFAULT = 10;

/** Clamp a typed value into the range the server accepts. */
export function clampEvery(n: number): number {
  if (!Number.isFinite(n)) return IMPROVEMENTS_DEFAULT;
  return Math.min(IMPROVEMENTS_MAX, Math.max(IMPROVEMENTS_MIN, Math.round(n)));
}

/**
 * Right drawer: switch a workflow's `improvements:` on or off, and set how many
 * finished runs per project sit between reviews.
 *
 * The block was round-tripped but invisible — kept on save, yet with no way to
 * tell from the UI whether a workflow had it on.
 */
export function WorkflowImprovementsDrawer({
  improvements,
  onChange,
  onClose,
}: {
  improvements: ImprovementsConfig | null | undefined;
  onChange: (next: ImprovementsConfig | undefined) => void;
  onClose: () => void;
}) {
  const on = !!improvements?.every;
  // A draft for the number, so typing "1" on the way to "15" isn't clamped
  // to 3 mid-keystroke. Committed (clamped) on blur.
  const [draft, setDraft] = useState(
    String(improvements?.every ?? IMPROVEMENTS_DEFAULT),
  );
  useEffect(() => {
    setDraft(String(improvements?.every ?? IMPROVEMENTS_DEFAULT));
  }, [improvements?.every]);

  const commit = () => {
    if (!on) return;
    const every = clampEvery(Number(draft));
    setDraft(String(every));
    if (every !== improvements?.every) onChange({ every });
  };

  return (
    <div className="flex w-1/3 min-w-[20rem] flex-none flex-col border-l border-border bg-card">
      <div className="flex items-center justify-between border-b border-border px-4 py-3">
        <span className="text-sm font-semibold">Improvements</span>
        <button
          type="button"
          onClick={onClose}
          className="rounded p-1 hover:bg-secondary"
          aria-label="Close"
        >
          <X className="h-4 w-4" />
        </button>
      </div>

      <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-auto p-4 text-[13px]">
        <p className="text-[12px] text-muted-foreground">
          After a set number of finished runs in a project, the harness reviews
          them and suggests changes: mostly to the project's repo (its{" "}
          <code>CLAUDE.md</code>, lint rules, skills), sometimes to this
          workflow. Nothing changes until a person applies a suggestion.
        </p>

        <div className="flex items-center justify-between gap-3">
          <Label htmlFor="improvements-on">Suggest improvements</Label>
          <Switch
            id="improvements-on"
            checked={on}
            onCheckedChange={(checked) =>
              onChange(
                checked
                  ? { every: clampEvery(Number(draft) || IMPROVEMENTS_DEFAULT) }
                  : undefined,
              )
            }
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <Label htmlFor="improvements-every">Review every</Label>
          <div className="flex items-center gap-2">
            <Input
              id="improvements-every"
              type="number"
              inputMode="numeric"
              min={IMPROVEMENTS_MIN}
              max={IMPROVEMENTS_MAX}
              className="w-24 tabular-nums"
              value={draft}
              disabled={!on}
              onChange={(e) => setDraft(e.target.value)}
              onBlur={commit}
              onKeyDown={(e) => {
                if (e.key === "Enter") commit();
              }}
            />
            <span className="text-[12px] text-muted-foreground">
              finished runs per project ({IMPROVEMENTS_MIN}–{IMPROVEMENTS_MAX})
            </span>
          </div>
          <p className="text-[11px] text-muted-foreground">
            Only completed or failed runs count. Cancelled runs and A/B arms do
            not, and each project is counted separately.
          </p>
        </div>

        <p className="text-[11px] text-muted-foreground">
          Saved with the workflow — press Save in the header to persist it.
        </p>
      </div>
    </div>
  );
}
