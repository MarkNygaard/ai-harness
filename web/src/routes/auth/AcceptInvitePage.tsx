import { useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useAcceptInvite, useInviteDetails } from "@/lib/invites";

const inputCls =
  "h-9 w-full rounded-md border border-input bg-transparent px-2.5 text-[13px] outline-none focus:ring-2 focus:ring-ring";

/**
 * Redeeming an invitation or a reset link.
 *
 * One page for both: an invitation creates the account, a reset repairs it, and
 * from here the only difference is the words and whether a name is asked for.
 */
export function AcceptInvitePage() {
  const { token } = useParams<{ token: string }>();
  const navigate = useNavigate();
  const details = useInviteDetails(token ?? null);
  const accept = useAcceptInvite(token ?? null);

  const [name, setName] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");

  const isReset = details.data?.kind === "reset";
  const mismatch = confirm.length > 0 && password !== confirm;

  return (
    <div className="flex min-h-svh items-center justify-center p-6">
      <div className="flex w-full max-w-sm flex-col gap-4">
        <h1 className="text-center text-lg font-semibold">
          {isReset ? "Set a new password" : "Welcome to ai-harness"}
        </h1>

        {details.isError && (
          <Card>
            <CardContent className="flex flex-col gap-3 px-4 py-4 text-xs">
              <p className="text-destructive">{details.error.message}</p>
              <p className="text-muted-foreground">
                Links work once and expire. Ask whoever invited you for a fresh
                one, or request another reset.
              </p>
              <Button size="sm" variant="outline" onClick={() => navigate("/")}>
                Back to the harness
              </Button>
            </CardContent>
          </Card>
        )}

        {accept.isSuccess && (
          <Card>
            <CardContent className="flex flex-col gap-3 px-4 py-4 text-xs">
              <p>
                {isReset
                  ? "Your password is set. Any other sessions were signed out."
                  : "Your account is ready."}
              </p>
              <Button size="sm" onClick={() => navigate("/login")}>
                Sign in
              </Button>
            </CardContent>
          </Card>
        )}

        {details.data && !accept.isSuccess && (
          <Card>
            <CardContent className="px-4 py-4">
              <form
                className="flex flex-col gap-3"
                onSubmit={(e) => {
                  e.preventDefault();
                  if (mismatch) return;
                  accept.mutate(
                    isReset ? { password } : { name: name.trim(), password },
                  );
                }}
              >
                <div className="text-[11px] text-muted-foreground">
                  {details.data.email}
                </div>

                {!isReset && (
                  <label className="flex flex-col gap-1">
                    <span className="text-[11px] font-medium text-muted-foreground">
                      Your name
                    </span>
                    <input
                      className={inputCls}
                      value={name}
                      onChange={(e) => setName(e.target.value)}
                      placeholder="How you appear to your team"
                      autoComplete="name"
                    />
                  </label>
                )}

                <label className="flex flex-col gap-1">
                  <span className="text-[11px] font-medium text-muted-foreground">
                    Password
                  </span>
                  <input
                    className={inputCls}
                    type="password"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    autoComplete="new-password"
                    required
                  />
                </label>

                <label className="flex flex-col gap-1">
                  <span className="text-[11px] font-medium text-muted-foreground">
                    Confirm
                  </span>
                  <input
                    className={inputCls}
                    type="password"
                    value={confirm}
                    onChange={(e) => setConfirm(e.target.value)}
                    autoComplete="new-password"
                    required
                  />
                </label>

                {mismatch && (
                  <span className="text-[11px] text-destructive">
                    Those do not match.
                  </span>
                )}
                {accept.isError && (
                  <span className="text-[11px] text-destructive">
                    {accept.error.message}
                  </span>
                )}

                <Button
                  type="submit"
                  disabled={!password || mismatch || accept.isPending}
                >
                  {accept.isPending
                    ? "Setting…"
                    : isReset
                      ? "Set password"
                      : "Create account"}
                </Button>
              </form>
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  );
}
