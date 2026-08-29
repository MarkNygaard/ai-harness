import { useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { PasswordField } from "@/components/auth/PasswordField";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useAcceptInvite, useInviteDetails } from "@/lib/invites";
import { useGithubSsoPublicStatus, useSsoPublicStatus } from "@/lib/sso";
import { useAuthStatus } from "@/lib/auth";

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

  const status = useAuthStatus();
  const [name, setName] = useState("");
  const [password, setPassword] = useState("");
  const [passwordOk, setPasswordOk] = useState(false);

  const sso = useSsoPublicStatus();
  const github = useGithubSsoPublicStatus();

  const isReset = details.data?.kind === "reset";
  const minLen = status.data?.min_password_len ?? 12;

  // Somebody who signs in through a provider does not need a password: it
  // would be invented here, never used, and stored anyway. Offered only when a
  // provider is actually armed — the server refuses otherwise, since the
  // account would have no way in at all.
  const providers = [
    github.data?.enabled ? "GitHub" : null,
    sso.data?.enabled ? (sso.data.label ?? "your identity provider") : null,
  ].filter(Boolean) as string[];
  const canSkipPassword = !isReset && providers.length > 0;
  const [wantsPassword, setWantsPassword] = useState(false);
  const showPassword = !canSkipPassword || wantsPassword;
  const ready = showPassword ? passwordOk : true;

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
                  : showPassword
                    ? "Your account is ready."
                    : `Your account is ready. Sign in with ${providers[0]}.`}
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
                  if (!ready) return;
                  accept.mutate(
                    isReset
                      ? { password }
                      : showPassword
                        ? { name: name.trim(), password }
                        : { name: name.trim() },
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

                {showPassword ? (
                  <PasswordField
                    value={password}
                    onChange={setPassword}
                    minLength={minLen}
                    // The address is already known from the link; the name is
                    // only asked for on an invitation.
                    identity={[details.data.email, name]}
                    onValidChange={setPasswordOk}
                  />
                ) : (
                  <p className="text-[11px] text-muted-foreground">
                    You will sign in with {providers.join(" or ")}, so there is
                    no password to choose.
                  </p>
                )}

                {canSkipPassword && (
                  <button
                    type="button"
                    className="self-start text-[11px] text-muted-foreground underline-offset-2 hover:underline"
                    onClick={() => setWantsPassword((v) => !v)}
                  >
                    {wantsPassword
                      ? `Use ${providers[0]} instead`
                      : "Set a password instead"}
                  </button>
                )}

                {accept.isError && (
                  <span className="text-[11px] text-destructive">
                    {accept.error.message}
                  </span>
                )}

                <Button type="submit" disabled={!ready || accept.isPending}>
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
