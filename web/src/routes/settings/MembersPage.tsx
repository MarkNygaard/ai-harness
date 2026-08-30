import { InvitePeople } from "@/components/settings/InvitePeople";
import { MemberProfileDialog } from "@/components/settings/MemberProfileDialog";
import { SettingsShell } from "@/components/SettingsShell";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useAuthStatus } from "@/lib/auth";
import type { AuthUser } from "@/lib/auth";
import {
  useDeleteUser,
  useSetUserDisabled,
  useSetUserRole,
  useUsers,
} from "@/lib/users";

function whenever(iso: string | null): string {
  if (!iso) return "never";
  const days = Math.floor((Date.now() - Date.parse(iso)) / 86_400_000);
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 30) return `${days} days ago`;
  return new Date(iso).toLocaleDateString();
}

function Row({
  user,
  isMe,
  admins,
  busy,
  onRole,
  onDisabled,
  onDelete,
}: {
  user: AuthUser;
  isMe: boolean;
  admins: number;
  busy: boolean;
  onRole: (role: "admin" | "member") => void;
  onDisabled: (disabled: boolean) => void;
  onDelete: () => void;
}) {
  const disabled = !!user.disabled_at;
  // Mirrors the server's guard so the button is disabled rather than the click
  // being refused — the server still decides, this only saves a round trip.
  const lastAdmin = user.role === "admin" && !disabled && admins <= 1;

  return (
    <div className="flex flex-col gap-2 border-t border-border px-4 py-3 first:border-t-0 sm:flex-row sm:items-center sm:justify-between">
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          <span className="truncate text-[13px] font-medium">{user.name}</span>
          {user.role === "admin" && (
            <Badge variant="secondary" className="text-[10px]">
              admin
            </Badge>
          )}
          {disabled && (
            <Badge variant="outline" className="text-[10px]">
              suspended
            </Badge>
          )}
          {isMe && (
            <span className="text-[10px] text-muted-foreground">you</span>
          )}
        </div>
        <div className="truncate text-[11px] text-muted-foreground">
          {user.email} · last signed in {whenever(user.last_login_at)}
        </div>
      </div>

      <div className="flex shrink-0 items-center gap-1">
        <MemberProfileDialog user={user} disabled={busy} />
        <Button
          variant="ghost"
          size="sm"
          disabled={busy || lastAdmin}
          title={
            lastAdmin
              ? "This is the only administrator — promote someone else first"
              : user.role === "admin"
                ? "Make this account a member"
                : "Make this account an administrator"
          }
          onClick={() => onRole(user.role === "admin" ? "member" : "admin")}
        >
          {user.role === "admin" ? "Demote" : "Make admin"}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          disabled={busy || isMe || lastAdmin}
          title={
            isMe
              ? "You cannot suspend your own account"
              : disabled
                ? "Let this account sign in again"
                : "Sign this account out and stop it signing back in"
          }
          onClick={() => onDisabled(!disabled)}
        >
          {disabled ? "Restore" : "Suspend"}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          className="text-muted-foreground"
          disabled={busy || isMe || lastAdmin}
          title={
            isMe
              ? "You cannot remove your own account"
              : "Remove this account permanently"
          }
          onClick={() => {
            if (
              window.confirm(
                `Remove ${user.name}? Their sessions end immediately and the account cannot be recovered.`,
              )
            ) {
              onDelete();
            }
          }}
        >
          Remove
        </Button>
      </div>
    </div>
  );
}

export function MembersPage() {
  const status = useAuthStatus();
  const mode = status.data?.mode;
  const me = status.data?.user;
  // Only worth asking in `accounts` mode: there are no accounts otherwise.
  const users = useUsers(mode === "accounts");
  const role = useSetUserRole();
  const disable = useSetUserDisabled();
  const remove = useDeleteUser();

  const busy = role.isPending || disable.isPending || remove.isPending;
  const error = role.error ?? disable.error ?? remove.error ?? users.error;
  const list = users.data ?? [];
  const admins = list.filter(
    (u) => u.role === "admin" && !u.disabled_at,
  ).length;

  return (
    <SettingsShell title="Members">
      <div className="mx-auto flex max-w-3xl flex-col gap-5 p-6">
        {mode !== "accounts" ? (
          <Card>
            <CardContent className="px-4 py-3 text-xs">
              This harness has no accounts yet, so there is nobody to manage.
              Anyone who can reach it can use it. Claiming it at{" "}
              <code className="font-mono">/setup</code> creates the first
              administrator and turns sign-in on — which cannot be turned off
              again.
            </CardContent>
          </Card>
        ) : (
          <>
            <p className="max-w-prose text-xs text-muted-foreground">
              Administrators can change credentials, sign-in, mail and who has
              an account. Members trigger runs, read reports and author
              workflows.
            </p>

            {error && (
              <p className="text-xs text-destructive">{error.message}</p>
            )}

            <Card>
              <CardContent className="p-0">
                {users.isLoading && (
                  <p className="px-4 py-3 text-xs text-muted-foreground">
                    Loading…
                  </p>
                )}
                {list.map((u) => (
                  <Row
                    key={u.id}
                    user={u}
                    isMe={u.id === me?.id}
                    admins={admins}
                    busy={busy}
                    onRole={(r) => role.mutate({ id: u.id, role: r })}
                    onDisabled={(d) =>
                      disable.mutate({ id: u.id, disabled: d })
                    }
                    onDelete={() => remove.mutate({ id: u.id })}
                  />
                ))}
              </CardContent>
            </Card>

            <InvitePeople />

            <p className="text-[11px] text-muted-foreground">
              An invitation always produces a link you can paste, whether or not
              mail is configured. Someone with a shell on the server can also
              add an account with{" "}
              <code className="font-mono">
                harness admin create --email … --password …
              </code>{" "}
              — which is how you get back in if you are locked out.
            </p>
          </>
        )}
      </div>
    </SettingsShell>
  );
}
