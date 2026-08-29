import { useState } from "react";
import { IconCopy, IconSend, IconTrash } from "@tabler/icons-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useCreateInvite, useInvites, useRevokeInvite } from "@/lib/invites";
import type { CreatedInvite } from "@/lib/invites";

const inputCls =
  "h-8 rounded-md border border-input bg-transparent px-2 text-[13px] outline-none focus:ring-2 focus:ring-ring";

/**
 * Inviting people, and the link that does it.
 *
 * The link is shown **in the same panel**, immediately, whether or not mail went
 * out — SMTP is configured elsewhere in these same settings, so an invite that
 * required it could never reach the person who would configure it.
 */
export function InvitePeople() {
  const invites = useInvites(true);
  const create = useCreateInvite();
  const revoke = useRevokeInvite();
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<"admin" | "member">("member");
  const [issued, setIssued] = useState<CreatedInvite | null>(null);

  const pending = invites.data ?? [];

  return (
    <section className="flex flex-col gap-2">
      <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        Invitations
      </h2>

      <Card>
        <CardContent className="p-0">
          {pending.map((i) => (
            <div
              key={i.id}
              className="flex items-center gap-2 border-t border-border px-4 py-2.5 first:border-t-0"
            >
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="truncate text-[13px]">{i.email}</span>
                  {i.role === "admin" && (
                    <Badge variant="secondary" className="text-[10px]">
                      admin
                    </Badge>
                  )}
                </div>
                <div className="text-[11px] text-muted-foreground">
                  expires {new Date(i.expires_at).toLocaleDateString()}
                </div>
              </div>
              <Button
                variant="ghost"
                size="sm"
                className="text-muted-foreground"
                disabled={revoke.isPending}
                title="Withdraw this invitation"
                onClick={() => revoke.mutate(i.id)}
              >
                <IconTrash className="size-3.5" />
                Withdraw
              </Button>
            </div>
          ))}

          {pending.length === 0 && !invites.isLoading && (
            <p className="px-4 py-3 text-[11px] text-muted-foreground">
              Nobody is waiting on an invitation.
            </p>
          )}

          <form
            className="flex flex-col gap-2 border-t border-border px-4 py-2.5 sm:flex-row sm:items-center"
            onSubmit={(e) => {
              e.preventDefault();
              const trimmed = email.trim();
              if (!trimmed) return;
              create.mutate(
                { email: trimmed, role },
                {
                  onSuccess: (data) => {
                    setEmail("");
                    setIssued(data);
                  },
                },
              );
            }}
          >
            <input
              className={`${inputCls} min-w-0 flex-1`}
              type="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              placeholder="them@example.com"
              aria-label="Email address to invite"
            />
            <select
              className={inputCls}
              value={role}
              onChange={(e) => setRole(e.target.value as "admin" | "member")}
              aria-label="Role"
            >
              <option value="member">Member</option>
              <option value="admin">Admin</option>
            </select>
            <Button
              type="submit"
              size="sm"
              variant="outline"
              disabled={!email.trim() || create.isPending}
            >
              <IconSend className="size-3.5" />
              {create.isPending ? "Inviting…" : "Invite"}
            </Button>
          </form>
        </CardContent>
      </Card>

      {issued && (
        <Card>
          <CardContent className="flex flex-col gap-2 px-4 py-3">
            <div className="text-[12px] font-medium">
              Invited {issued.invite.email}
            </div>
            {issued.link ? (
              <>
                <div className="flex items-center gap-2">
                  <code className="min-w-0 flex-1 truncate rounded bg-muted px-2 py-1 font-mono text-[11px]">
                    {issued.link}
                  </code>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => navigator.clipboard?.writeText(issued.link!)}
                  >
                    <IconCopy className="size-3.5" />
                    Copy
                  </Button>
                </div>
                <span className="text-[11px] text-muted-foreground">
                  {issued.mailed
                    ? "Also sent by email. Send them the link too if it does not arrive."
                    : "Send them this link — it works once and expires in a week."}
                </span>
                {issued.mail_error && (
                  <span className="text-[11px] text-destructive">
                    Mail did not go out: {issued.mail_error}
                  </span>
                )}
              </>
            ) : (
              <span className="text-[11px] text-destructive">
                The invitation exists, but there is no public URL set, so there
                is no link to give them. Set one under General.
              </span>
            )}
          </CardContent>
        </Card>
      )}

      {(create.isError || revoke.isError || invites.isError) && (
        <span className="text-[11px] text-destructive">
          {create.error?.message ??
            revoke.error?.message ??
            invites.error?.message}
        </span>
      )}
    </section>
  );
}
