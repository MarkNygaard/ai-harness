import { useState } from "react";
import { BookmarkPlus } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { useSaveStep } from "@/lib/authoring";
import { nodeToDoc } from "@/lib/workflow-yaml";
import type { EditorNode } from "@/types/authoring";

/**
 * "Save as step": keep this node, with all its settings, as a reusable step in
 * everyone's palette. It is saved as a copy, so editing this node afterwards,
 * or deleting the saved step, changes nothing else.
 */
export function SaveStepButton({ node }: { node: EditorNode }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button
        size="sm"
        variant="ghost"
        onClick={() => setOpen(true)}
        title="Keep this step to reuse in other workflows"
      >
        <BookmarkPlus />
        Save as step
      </Button>
      {open && <SaveStepDialog node={node} onClose={() => setOpen(false)} />}
    </>
  );
}

function SaveStepDialog({
  node,
  onClose,
}: {
  node: EditorNode;
  onClose: () => void;
}) {
  const save = useSaveStep();
  const [label, setLabel] = useState(node.id);
  const [description, setDescription] = useState("");

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Save as step</DialogTitle>
          <DialogDescription>
            It shows up under Saved steps in everyone's palette. Inserting it
            adds a copy, so later changes to it don't affect workflows that
            already use it.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (!label.trim()) return;
            save.mutate(
              {
                label: label.trim(),
                description: description.trim(),
                node: nodeToDoc(node),
              },
              { onSuccess: onClose },
            );
          }}
        >
          <label className="flex flex-col gap-1">
            <span className="text-[11px] font-medium text-muted-foreground">
              Name
            </span>
            <Input
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder="File report in Linear"
              autoFocus
            />
          </label>
          <label className="flex flex-col gap-1">
            <span className="text-[11px] font-medium text-muted-foreground">
              What it does (optional)
            </span>
            <Textarea
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="Files $ARTIFACTS_DIR/report.md as a Linear issue in ECOM."
              className="min-h-20"
            />
          </label>
          <p className="text-[11px] text-muted-foreground">
            Its connections aren't saved: you connect it again wherever you
            insert it.
          </p>
          {save.isError && (
            <p className="text-[12px] text-destructive">{save.error.message}</p>
          )}
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" disabled={!label.trim() || save.isPending}>
              {save.isPending ? "Saving…" : "Save step"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
