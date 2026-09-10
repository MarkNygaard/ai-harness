import { useEffect, useState } from "react";
import { IconCloudUpload } from "@tabler/icons-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { useAuthStatus } from "@/lib/auth";
import { usePublishWorkflow, usePublisher } from "@/lib/library";
import { titleFromSlug } from "@/lib/workflow-name";
import type { WorkflowSummary } from "@/types/authoring";

/**
 * Share a workflow you wrote, or publish a new version of one you already
 * shared.
 *
 * One button for both. Which it is comes from the harness's own record, not
 * from a choice offered here — the two wrong answers are a duplicate entry and
 * a version aimed at somebody else's workflow, and neither is worth risking to
 * save a decision nobody wants to make.
 */
export function PublishDialog({ wf }: { wf: WorkflowSummary }) {
  const [open, setOpen] = useState(false);
  const republish = wf.installed?.published === true;

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger
        render={
          <Button variant="outline" size="sm">
            <IconCloudUpload className="size-4" />
            {republish ? "Publish update" : "Publish"}
          </Button>
        }
      />
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>
            {republish ? "Publish an update" : "Publish to the library"}
          </DialogTitle>
        </DialogHeader>
        {/* Mounted only while open, so the identity lookup — which reaches the
            registry — happens when somebody asks to publish rather than on
            every render of the workflows page. */}
        {open && (
          <PublishForm
            wf={wf}
            republish={republish}
            onDone={() => setOpen(false)}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

function PublishForm({
  wf,
  republish,
  onDone,
}: {
  wf: WorkflowSummary;
  republish: boolean;
  onDone: () => void;
}) {
  const auth = useAuthStatus();
  const publisher = usePublisher(true);
  const publish = usePublishWorkflow();

  const [title, setTitle] = useState(() => titleFromSlug(wf.name));
  const [description, setDescription] = useState(wf.description ?? "");
  const [changelog, setChangelog] = useState("");
  const [publishAs, setPublishAs] = useState("");

  // Prefilled once the registry says who the token is, and only when it has no
  // name of its own yet. A harness account carries no GitHub identity, so the
  // login on the row is whatever an operator recorded when minting the token —
  // often not what this person would choose to be called in a public library.
  // Their harness name is the better guess, and it stays editable either way.
  //
  // Not overwritten on later renders: typing here must not be undone by a
  // refetch resolving.
  const known = publisher.data?.name ?? null;
  const suggested = auth.data?.user?.name ?? "";
  useEffect(() => {
    if (publisher.isSuccess && !publishAs) setPublishAs(known || suggested);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [publisher.isSuccess]);

  if (publisher.isLoading) {
    return (
      <p className="text-sm text-muted-foreground">
        Checking your publisher token…
      </p>
    );
  }
  if (publisher.isError) {
    return (
      <p className="text-sm text-destructive">{publisher.error.message}</p>
    );
  }
  if (!publisher.data?.configured) {
    return (
      <div className="flex flex-col gap-2 text-sm">
        <p className="text-muted-foreground">
          Publishing needs a publisher token, which the person running the
          library issues. Add it under{" "}
          <span className="font-medium text-foreground">
            Settings → Integrations
          </span>
          , then publish from here.
        </p>
      </div>
    );
  }

  const submit = () => {
    publish.mutate(
      {
        name: wf.name,
        title: title.trim() || undefined,
        description: description.trim() || undefined,
        changelog: changelog.trim() || undefined,
        publish_as: publishAs.trim() || undefined,
      },
      { onSuccess: onDone },
    );
  };

  return (
    <div className="flex flex-col gap-3 text-sm">
      <p className="text-muted-foreground">
        {republish ? (
          <>
            A new version of{" "}
            <span className="font-mono text-xs text-foreground">{wf.name}</span>
            . Installs are told an update is available; nothing changes under
            them until they take it.
          </>
        ) : (
          <>
            Shares{" "}
            <span className="font-mono text-xs text-foreground">{wf.name}</span>{" "}
            as it is on this harness. Anyone can install a copy; yours is never
            changed by theirs.
          </>
        )}
      </p>

      <Field label="Published as">
        <Input
          value={publishAs}
          onChange={(e) => setPublishAs(e.target.value)}
          placeholder={suggested || "Your name"}
        />
        <p className="text-[11px] text-muted-foreground">
          The name shown on every workflow you publish, not just this one.
        </p>
      </Field>

      {!republish && (
        <>
          <Field label="Title">
            <Input value={title} onChange={(e) => setTitle(e.target.value)} />
          </Field>
          <Field label="Description">
            <Textarea
              rows={3}
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="What it does, and who it is for."
            />
          </Field>
        </>
      )}

      <Field label={republish ? "What changed" : "Release note"}>
        <Textarea
          rows={2}
          value={changelog}
          onChange={(e) => setChangelog(e.target.value)}
          placeholder={republish ? "Fixed the review step." : "Optional."}
        />
      </Field>

      {publish.isError && (
        <p className="text-sm text-destructive">{publish.error.message}</p>
      )}

      <div className="flex justify-end gap-2 pt-1">
        <Button variant="ghost" size="sm" onClick={onDone}>
          Cancel
        </Button>
        <Button size="sm" onClick={submit} disabled={publish.isPending}>
          {publish.isPending
            ? "Publishing…"
            : republish
              ? "Publish update"
              : "Publish"}
        </Button>
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
    <div className="flex flex-col gap-1">
      <span className="text-[11px] font-medium text-muted-foreground">
        {label}
      </span>
      {children}
    </div>
  );
}
