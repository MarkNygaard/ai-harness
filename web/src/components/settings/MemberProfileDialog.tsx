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
import type { ProfileUpdate } from "@/lib/users";
import { useSetUserProfile } from "@/lib/users";

export function MemberProfileDialog({
  user,
  busy,
  isMe,
  onSaved,
}: {
  user: AuthUser;
  busy: boolean;
  isMe: boolean;
  onSaved: (result: ProfileUpdate) => void;
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
    if (save.isPending || !n || !m) return;
    save.mutate(
      { id: user.id, name: n, email: m },
      {
        onSuccess: (result) => {
          onSaved(result);
          onOpenChange(false);
        },
      },
    );
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next, details) => {
        // Keep the modal boundary intact until the request settles. Otherwise
        // closing and reopening can start a second write while the first is
        // still in flight, and the responses can apply out of order.
        if (!next && save.isPending) {
          details.cancel();
          return;
        }
        onOpenChange(next);
      }}
    >
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
      <DialogContent showCloseButton={!save.isPending}>
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
          {email.trim().toLowerCase() !== user.email.trim().toLowerCase() && (
            <p className="text-[11px] text-muted-foreground">
              {isMe
                ? "Changing your own address ends this session too — you will have to sign in again with the new address."
                : `Changing the address ends every session this account holds, so ${user.name} will have to sign in again.`}
            </p>
          )}
          {save.isError && (
            <p role="alert" className="text-[11px] text-destructive">
              {save.error.message}
            </p>
          )}
          <DialogFooter>
            <DialogClose
              render={
                <Button
                  type="button"
                  variant="outline"
                  disabled={save.isPending}
                />
              }
            >
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
