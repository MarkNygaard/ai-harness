/**
 * Edit a member's name and email from the Members page.
 *
 * The row actions used to have no way to fix a wrong name or a changed
 * address; this dialog owns its own mutation and keeps the pending/error
 * state inside the modal so the 409 duplicate-email sentence is read where
 * the person is looking.
 */
import { useId, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { AuthUser } from "@/lib/auth";
import { useSetUserProfile } from "@/lib/users";

export function MemberProfileDialog({
  user,
  busy,
}: {
  user: AuthUser;
  busy: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState(user.name);
  const [email, setEmail] = useState(user.email);
  const save = useSetUserProfile();
  const uid = useId();
  const nameId = `${uid}-name`;
  const emailId = `${uid}-email`;

  function onOpenChange(next: boolean) {
    setOpen(next);
    // Reseed from the prop on every open/close: a background ["users"]
    // refetch must not clobber what the person is typing while the dialog is
    // open, and a stale error from a previous attempt must not still be
    // showing when the dialog reopens.
    setName(user.name);
    setEmail(user.email);
    save.reset();
  }

  function submit(e: React.FormEvent) {
    e.preventDefault();
    const n = name.trim();
    const m = email.trim();
    if (!n || !m) return;
    save.mutate(
      { id: user.id, name: n, email: m },
      { onSuccess: () => onOpenChange(false) },
    );
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogTrigger
        render={
          <Button
            variant="ghost"
            size="sm"
            disabled={busy}
            title="Change this member's name and email"
          />
        }
      >
        Edit
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Edit member</DialogTitle>
          <DialogDescription>
            The address is what they sign in with, and only one account can hold
            it.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-3"
          onSubmit={submit}
          // Disable the browser's native email bubble so the server remains the
          // authority on what a valid, free address is.
          noValidate
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={nameId}>Name</Label>
            <Input
              id={nameId}
              autoComplete="name"
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={emailId}>Email</Label>
            <Input
              id={emailId}
              type="email"
              autoComplete="email"
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
            />
          </div>
          {save.isError && (
            <p role="alert" className="text-[11px] text-destructive">
              {save.error.message}
            </p>
          )}
          <DialogFooter>
            <DialogClose render={<Button type="button" variant="outline" />}>
              Cancel
            </DialogClose>
            {/* Mirrors the server's empty-field 400s so the Save is disabled
                rather than the request being refused — the server still
                decides. */}
            <Button
              type="submit"
              disabled={!name.trim() || !email.trim() || save.isPending}
            >
              {save.isPending ? "Saving…" : "Save changes"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
