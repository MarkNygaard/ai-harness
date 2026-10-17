/**
 * Limit a member to a set of projects, from the Members page.
 *
 * The control is two radio choices rather than a bare checklist, because the
 * empty case is the one people get wrong: no projects ticked means **every**
 * project, not none. A checklist alone would say the opposite of what it does.
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
import { useProjects } from "@/lib/projects";
import { useSetUserProjects } from "@/lib/users";
import type { UserActivity } from "@/lib/users";

export function MemberProjectsDialog({
  user,
  busy,
}: {
  user: UserActivity;
  busy: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [limited, setLimited] = useState(user.projects.length > 0);
  const [picked, setPicked] = useState<string[]>(user.projects);
  const projects = useProjects();
  const save = useSetUserProjects();
  const uid = useId();

  const isAdmin = user.role === "admin";
  // An empty pick while "only these" is selected would *clear* the limit
  // rather than apply it — the one input here that would quietly do the
  // opposite of what it looks like.
  const nothingPicked = limited && picked.length === 0;

  function reseed(next: boolean) {
    setOpen(next);
    // Reseed on every open and close: a background ["users"] refetch must not
    // replace what the administrator is in the middle of choosing, and a stale
    // error must not still be showing when the dialog reopens.
    setLimited(user.projects.length > 0);
    setPicked(user.projects);
    save.reset();
  }

  function toggle(name: string) {
    setPicked((prev) =>
      prev.includes(name) ? prev.filter((p) => p !== name) : [...prev, name],
    );
  }

  function submit(e: React.FormEvent) {
    e.preventDefault();
    if (save.isPending || nothingPicked) return;
    save.mutate(
      { id: user.id, projects: limited ? picked : [] },
      { onSuccess: () => reseed(false) },
    );
  }

  // A saved scope can name a project that has since been deleted, or one from
  // another install. Show it rather than dropping it silently on the next save.
  const unregistered = picked.filter(
    (n) => !projects.data?.some((p) => p.name === n),
  );

  return (
    <Dialog
      open={open}
      onOpenChange={(next, details) => {
        // Hold the modal open until the write settles, so closing and
        // reopening cannot start a second one that lands out of order.
        if (!next && save.isPending) {
          details.cancel();
          return;
        }
        reseed(next);
      }}
    >
      <DialogTrigger
        render={
          <Button
            variant="ghost"
            size="sm"
            disabled={busy || isAdmin}
            title={
              isAdmin
                ? "Administrators always see every project — demote this account first"
                : "Choose which projects this member can see"
            }
          />
        }
      >
        Projects
      </DialogTrigger>
      <DialogContent showCloseButton={!save.isPending}>
        <DialogHeader>
          <DialogTitle>Projects for {user.name}</DialogTitle>
          <DialogDescription>
            What this member can see and start runs on. Everything else is
            hidden from them, as though it were not here.
          </DialogDescription>
        </DialogHeader>
        <form className="flex flex-col gap-3" onSubmit={submit}>
          <label className="flex items-start gap-2 text-[13px]">
            <input
              type="radio"
              name={`${uid}-scope`}
              className="mt-1"
              checked={!limited}
              onChange={() => setLimited(false)}
            />
            <span>
              <span className="font-medium">Every project</span>
              <span className="block text-[11px] text-muted-foreground">
                Including any project added later.
              </span>
            </span>
          </label>

          <label className="flex items-start gap-2 text-[13px]">
            <input
              type="radio"
              name={`${uid}-scope`}
              className="mt-1"
              checked={limited}
              onChange={() => setLimited(true)}
            />
            <span>
              <span className="font-medium">Only the projects I pick</span>
              <span className="block text-[11px] text-muted-foreground">
                A project added later stays hidden until you add it here.
              </span>
            </span>
          </label>

          {limited && (
            <div className="ml-6 flex max-h-56 flex-col gap-1 overflow-y-auto rounded-md border border-border p-2">
              {projects.isLoading && (
                <span className="text-[11px] text-muted-foreground">
                  Loading projects…
                </span>
              )}
              {projects.data?.length === 0 && (
                <span className="text-[11px] text-muted-foreground">
                  No projects registered yet.
                </span>
              )}
              {projects.data?.map((p) => (
                <label
                  key={p.name}
                  className="flex items-center gap-2 text-[13px]"
                >
                  <input
                    type="checkbox"
                    checked={picked.includes(p.name)}
                    onChange={() => toggle(p.name)}
                  />
                  {p.name}
                </label>
              ))}
              {unregistered.map((name) => (
                <label
                  key={name}
                  className="flex items-center gap-2 text-[13px] text-muted-foreground"
                >
                  <input
                    type="checkbox"
                    checked
                    onChange={() => toggle(name)}
                  />
                  {name} (not registered)
                </label>
              ))}
            </div>
          )}

          {nothingPicked && (
            <p className="text-[11px] text-muted-foreground">
              Pick at least one project, or choose “Every project”.
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
            <Button type="submit" disabled={save.isPending || nothingPicked}>
              {save.isPending ? "Saving…" : "Save changes"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
