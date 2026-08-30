import { useId, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { AuthUser } from "@/lib/auth";
import { useSetUserProfile } from "@/lib/users";

/**
 * Edit a member's name and email.
 *
 * The email is the account's identity — sign-in matches on it — so changing it
 * to one that already belongs to another account is a 409 conflict the person
 * must read rather than a silent overwrite.
 */
export function MemberProfileDialog({
  user,
  disabled,
}: {
  user: AuthUser;
  disabled: boolean;
}) {
  const save = useSetUserProfile();
  const [open, setOpen] = useState(false);
  const [name, setName] = useState(user.name);
  const [email, setEmail] = useState(user.email);
  const id = useId();
  const trimmedName = name.trim();
  const trimmedEmail = email.trim();

  function onOpenChange(next: boolean) {
    // Re-seed from the current prop on open. The component stays mounted
    // across refetches, so using `useState(user.name)` alone would leave the
    // second open showing stale values after a successful save. Syncing on
    // every prop change would overwrite what the person is typing while the
    // dialog is open, so we only reset here.
    if (next) {
      setName(user.name);
      setEmail(user.email);
    }
    // Clear any leftover error from the previous open/close cycle.
    save.reset();
    setOpen(next);
  }

  function submit() {
    if (!trimmedName || !trimmedEmail) return;

    save.mutate(
      { id: user.id, name: trimmedName, email: trimmedEmail },
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
            disabled={disabled}
            title="Change this account's name and email"
          />
        }
      >
        Edit
      </DialogTrigger>
      <DialogContent className="sm:max-w-sm">
        <DialogHeader>
          <DialogTitle>Edit member</DialogTitle>
          <DialogDescription>
            The email is what they sign in with, so changing it changes their
            sign-in.
          </DialogDescription>
        </DialogHeader>

        <form
          noValidate
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            submit();
          }}
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={`${id}-name`}>Name</Label>
            <Input
              id={`${id}-name`}
              value={name}
              onChange={(e) => setName(e.target.value)}
              autoComplete="name"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <Label htmlFor={`${id}-email`}>Email</Label>
            <Input
              id={`${id}-email`}
              type="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              autoComplete="email"
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
            />
          </div>

          <div className="flex flex-col items-end gap-1.5">
            <Button
              type="submit"
              disabled={!trimmedName || !trimmedEmail || save.isPending}
            >
              {save.isPending ? "Saving…" : "Save"}
            </Button>
            {save.isError && (
              <span className="text-[11px] text-destructive">
                {save.error.message}
              </span>
            )}
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}
