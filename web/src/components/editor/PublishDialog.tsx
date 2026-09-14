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
import {
  amendmentsFor,
  useAmendPublished,
  usePublishWorkflow,
  usePublisher,
  useUnpublishWorkflow,
} from "@/lib/library";
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
  const amend = useAmendPublished();
  const unpublish = useUnpublishWorkflow();

  // A published workflow keeps the title its author gave it, which is not
  // necessarily one derived from the file name. Seeding from the recorded title
  // is what stops "Publish update" quietly renaming the entry back.
  const seededTitle = wf.installed?.title || titleFromSlug(wf.name);
  const seededDescription = wf.description ?? "";

  const [title, setTitle] = useState(seededTitle);
  const [description, setDescription] = useState(seededDescription);
  const [changelog, setChangelog] = useState("");
  const [publishAs, setPublishAs] = useState("");
  const [confirmRemove, setConfirmRemove] = useState(false);

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
          {publisher.data?.enrollment
            ? "Publishing needs a one-off sign-in with GitHub, which the library uses to confirm who an entry belongs to."
            : "Publishing needs a publisher token, which the person running the library issues."}{" "}
          Connect it under{" "}
          <span className="font-medium text-foreground">
            Settings → Integrations
          </span>
          , then publish from here.
        </p>
      </div>
    );
  }

  const submit = async () => {
    // On a republish the entry already exists, so its title and description are
    // amended rather than sent with the version: the registry takes them on
    // create only, and a version is not the place to change what the entry says
    // about itself. Sent only when they actually changed, so the ordinary
    // "publish an update" stays one request.
    if (republish) {
      const amendments = amendmentsFor(
        { title, description },
        { title: seededTitle, description: seededDescription },
      );
      if (Object.keys(amendments).length > 0) {
        try {
          await amend.mutateAsync({ name: wf.name, ...amendments });
        } catch {
          // The mutation carries the message; publishing anyway would report
          // success for a half-applied change.
          return;
        }
      }
    }

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

      {/* Editable on a republish too. The entry's title and description are
          what people read in the library, and before this the only way to fix
          one was to publish the workflow again — which tells everyone holding
          it that something changed when nothing did. */}
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
      {amend.isError && (
        <p className="text-sm text-destructive">{amend.error.message}</p>
      )}
      {unpublish.isError && (
        <p className="text-sm text-destructive">{unpublish.error.message}</p>
      )}

      <div className="flex items-center gap-2 pt-1">
        {republish && <WithdrawControl />}
        <Button variant="ghost" size="sm" className="ml-auto" onClick={onDone}>
          Cancel
        </Button>
        <Button
          size="sm"
          onClick={submit}
          disabled={publish.isPending || amend.isPending}
        >
          {publish.isPending || amend.isPending
            ? "Publishing…"
            : republish
              ? "Publish update"
              : "Publish"}
        </Button>
      </div>
    </div>
  );

  /**
   * Take the entry out of the library.
   *
   * Two presses, because it is the one action here that other people can see
   * the result of. It removes nothing locally and breaks nobody's install: the
   * copy they took is theirs and goes on working, and publishing again puts the
   * entry back.
   */
  function WithdrawControl() {
    if (!confirmRemove) {
      return (
        <Button
          variant="ghost"
          size="sm"
          className="text-destructive"
          onClick={() => setConfirmRemove(true)}
        >
          Remove from library
        </Button>
      );
    }
    return (
      <div className="flex items-center gap-2">
        <Button
          variant="destructive"
          size="sm"
          disabled={unpublish.isPending}
          onClick={() => unpublish.mutate(wf.name, { onSuccess: onDone })}
        >
          {unpublish.isPending ? "Removing…" : "Really remove"}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => setConfirmRemove(false)}
        >
          Keep
        </Button>
      </div>
    );
  }
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
