import { X } from "lucide-react";
import { Textarea } from "@/components/ui/textarea";

/**
 * Right drawer: read and edit a workflow's `description`.
 *
 * The description was previously round-tripped but invisible — loaded into the
 * editor's state and written back on save, yet with no way to read or change it
 * outside the YAML. It is the text the Workflows list shows on every card, so it
 * earns a place in the builder.
 */
export function WorkflowDescriptionDrawer({
  description,
  onChange,
  onClose,
}: {
  description: string | undefined;
  onChange: (next: string | undefined) => void;
  onClose: () => void;
}) {
  const value = description ?? "";

  return (
    <div className="flex w-1/3 min-w-[20rem] flex-none flex-col border-l border-border bg-card">
      <div className="flex items-center justify-between border-b border-border px-4 py-3">
        <span className="text-sm font-semibold">Description</span>
        <button
          type="button"
          onClick={onClose}
          className="rounded p-1 hover:bg-secondary"
        >
          <X className="h-4 w-4" />
        </button>
      </div>

      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto p-4 text-[13px]">
        <p className="text-[12px] text-muted-foreground">
          What this workflow does. Shown on its card in the Workflows list, so
          lead with a one-line summary — blank lines start new paragraphs, and
          the card shows as much as fits.
        </p>
        <Textarea
          value={value}
          onChange={(e) =>
            // Empty → `undefined`, so an emptied description is dropped from the
            // YAML rather than written as an empty string.
            onChange(e.target.value.trim() === "" ? undefined : e.target.value)
          }
          placeholder={
            "The default pipeline: turn a task into a reviewed PR.\n\n" +
            "Flow: plan, implement, review, build gate."
          }
          className="min-h-64 flex-1 resize-none font-mono text-[12px] leading-relaxed"
          spellCheck
        />
        <p className="text-[11px] text-muted-foreground">
          Saved with the workflow — press Save in the header to persist it.
        </p>
      </div>
    </div>
  );
}
