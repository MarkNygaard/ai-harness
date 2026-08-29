import { useEffect, useState } from "react";
import { Link, Navigate, useNavigate } from "react-router-dom";
import { IconBrandGithub, IconHexagonalPrism } from "@tabler/icons-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useAuthStatus, useLogin, useSetup } from "@/lib/auth";
import {
  useGithubSsoPublicStatus,
  useSsoPublicStatus,
  useStartGithubSso,
  useStartSso,
} from "@/lib/sso";

const inputCls =
  "h-9 w-full rounded-md border border-input bg-transparent px-2.5 text-[13px] outline-none focus:ring-2 focus:ring-ring";

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[11px] font-medium text-muted-foreground">
        {label}
      </span>
      {children}
      {hint && (
        <span className="text-[10px] text-muted-foreground">{hint}</span>
      )}
    </label>
  );
}

/** The centred card both pages sit in — no sidebar, nothing else to click. */
function Frame({
  title,
  description,
  children,
}: {
  title: string;
  description: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex min-h-svh items-center justify-center bg-background p-6">
      <div className="flex w-full max-w-sm flex-col gap-4">
        <div className="flex items-center gap-2">
          <IconHexagonalPrism className="size-5" />
          <span className="text-base font-semibold">ai-harness</span>
        </div>
        <Card>
          <CardContent className="flex flex-col gap-4 px-5 py-5">
            <div>
              <h1 className="text-sm font-semibold">{title}</h1>
              <p className="mt-1 text-[11px] text-muted-foreground">
                {description}
              </p>
            </div>
            {children}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}

export function LoginPage() {
  const status = useAuthStatus();
  const login = useLogin();
  const sso = useSsoPublicStatus();
  const startSso = useStartSso();
  const github = useGithubSsoPublicStatus();
  const startGithub = useStartGithubSso();
  const navigate = useNavigate();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");

  // Already signed in, or this harness has no accounts at all — either way
  // there is nothing to do here.
  if (status.data?.user) return <Navigate to="/" replace />;
  if (status.data && !status.data.claimed)
    return <Navigate to="/setup" replace />;

  return (
    <Frame
      title="Sign in"
      description="Use the email and password for your account on this harness."
    >
      <form
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          login.mutate(
            { email, password },
            { onSuccess: () => navigate("/", { replace: true }) },
          );
        }}
      >
        <Field label="Email">
          <input
            className={inputCls}
            type="email"
            autoComplete="username"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            required
          />
        </Field>
        <Field label="Password">
          <input
            className={inputCls}
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            required
          />
        </Field>
        {login.isError && (
          <p className="text-[11px] text-destructive">{login.error.message}</p>
        )}
        <Button type="submit" disabled={login.isPending}>
          {login.isPending ? "Signing in…" : "Sign in"}
        </Button>
        {(sso.data?.enabled || github.data?.enabled) && (
          <>
            <div className="flex items-center gap-2 py-0.5">
              <span className="h-px flex-1 bg-border" />
              <span className="text-[10px] text-muted-foreground">or</span>
              <span className="h-px flex-1 bg-border" />
            </div>
            {sso.data?.enabled && (
              <Button
                type="button"
                variant="outline"
                disabled={startSso.isPending}
                onClick={() => startSso.mutate({ next: "/" })}
              >
                {startSso.isPending
                  ? "Redirecting…"
                  : `Continue with ${sso.data.label?.trim() || "your provider"}`}
              </Button>
            )}
            {github.data?.enabled && (
              <Button
                type="button"
                variant="outline"
                disabled={startGithub.isPending}
                onClick={() => startGithub.mutate({ next: "/" })}
              >
                <IconBrandGithub className="size-4" />
                {startGithub.isPending
                  ? "Redirecting…"
                  : "Continue with GitHub"}
              </Button>
            )}
            {(startSso.isError || startGithub.isError) && (
              <p className="text-[11px] text-destructive">
                {startSso.error?.message ?? startGithub.error?.message}
              </p>
            )}
          </>
        )}
        <Link
          to="/forgot"
          className="text-center text-[11px] text-muted-foreground underline"
        >
          Forgot your password?
        </Link>
      </form>
    </Frame>
  );
}

export function SetupPage() {
  const status = useAuthStatus();
  const setup = useSetup();
  const navigate = useNavigate();
  const [token, setToken] = useState("");
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const minLen = status.data?.min_password_len ?? 12;

  // Claiming is a one-way door, so once it is shut this page is meaningless.
  useEffect(() => {
    if (status.data?.claimed) navigate("/login", { replace: true });
  }, [status.data?.claimed, navigate]);

  return (
    <Frame
      title="Claim this harness"
      description="Nobody has an account here yet. The server printed a one-time setup token in its log when it started — paste it below to create the first administrator."
    >
      <form
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          setup.mutate(
            { setup_token: token, name, email, password },
            { onSuccess: () => navigate("/", { replace: true }) },
          );
        }}
      >
        <Field
          label="Setup token"
          hint="From the server log, or the .harness-setup-token file beside its data."
        >
          <input
            className={`${inputCls} font-mono`}
            value={token}
            onChange={(e) => setToken(e.target.value)}
            required
          />
        </Field>
        <Field label="Your name">
          <input
            className={inputCls}
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
          />
        </Field>
        <Field label="Email">
          <input
            className={inputCls}
            type="email"
            autoComplete="username"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            required
          />
        </Field>
        <Field label="Password" hint={`At least ${minLen} characters.`}>
          <input
            className={inputCls}
            type="password"
            autoComplete="new-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            required
            minLength={minLen}
          />
        </Field>
        {setup.isError && (
          <p className="text-[11px] text-destructive">{setup.error.message}</p>
        )}
        <p className="text-[10px] text-muted-foreground">
          After this, signing in is required and cannot be turned off again.
        </p>
        <Button type="submit" disabled={setup.isPending}>
          {setup.isPending ? "Claiming…" : "Claim and sign in"}
        </Button>
      </form>
    </Frame>
  );
}
