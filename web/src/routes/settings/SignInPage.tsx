import { useEffect, useState } from "react";
import { IconCopy, IconPlugConnected } from "@tabler/icons-react";
import { SettingsShell } from "@/components/SettingsShell";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { GithubSso } from "@/components/settings/GithubSso";
import {
  useSaveSso,
  useSsoConfig,
  useSsoOutcome,
  useTestSso,
} from "@/lib/sso";
import type { SsoInput } from "@/lib/sso";

const inputCls =
  "h-8 w-full rounded-md border border-input bg-transparent px-2 text-[13px] outline-none focus:ring-2 focus:ring-ring";

function Field({
  label,
  help,
  children,
}: {
  label: string;
  help?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[11px] font-medium text-muted-foreground">
        {label}
      </span>
      {children}
      {help && (
        <span className="text-[10px] text-muted-foreground">{help}</span>
      )}
    </label>
  );
}


export function SsoSettingsPage() {
  const config = useSsoConfig(true);
  const save = useSaveSso();
  const test = useTestSso();
  const outcome = useSsoOutcome();

  const [form, setForm] = useState<SsoInput>({});
  const [secret, setSecret] = useState("");
  const [seeded, setSeeded] = useState(false);
  const data = config.data;

  useEffect(() => {
    if (!data || seeded) return;
    setForm({
      issuer: data.issuer ?? "",
      client_id: data.client_id ?? "",
      allowed_domains: data.allowed_domains ?? "",
      label: data.label ?? "",
    });
    setSeeded(true);
  }, [data, seeded]);

  const set = <K extends keyof SsoInput>(key: K, value: SsoInput[K]) =>
    setForm((f) => ({ ...f, [key]: value }));

  const ready = !!data?.issuer && !!data?.client_id && data?.client_secret_set;

  return (
    <SettingsShell title="Sign-in">
      <div className="mx-auto flex max-w-3xl flex-col gap-5 p-6">
        <p className="max-w-prose text-xs text-muted-foreground">
          Let people sign in with your identity provider — anything that speaks
          OIDC discovery: Entra, Google, Okta, Keycloak, Authentik. Configured
          by issuer URL, so there is nothing provider-specific to pick.
        </p>

        <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          Identity provider
        </h2>

        <Card>
          <CardContent className="flex flex-col gap-3 px-4 py-3.5">
            <div className="flex items-center gap-2">
              <span className="text-[13px] font-medium">
                {data?.label?.trim() || "Identity provider"}
              </span>
              <Badge
                variant={data?.enabled ? "success" : "outline"}
                className="text-[10px]"
              >
                {data?.enabled ? "in use" : "not switched on"}
              </Badge>
            </div>

            {outcome?.status === "tested" && (
              <span className="text-[11px] text-status-success">
                That worked — the provider is now offered on the sign-in page.
              </span>
            )}
            {outcome?.status === "error" && (
              <span className="text-[11px] text-destructive">
                {outcome.message ?? "That did not work."}
              </span>
            )}
            {outcome?.status === "denied" && (
              <span className="text-[11px] text-muted-foreground">
                The provider declined: {outcome.message ?? "consent refused"}.
              </span>
            )}

            {data?.callback_url ? (
              <div className="flex items-center gap-2">
                <span className="w-24 shrink-0 text-[10px] text-muted-foreground">
                  Redirect URI
                </span>
                <code className="min-w-0 flex-1 truncate rounded bg-muted px-1.5 py-0.5 font-mono text-[10px]">
                  {data.callback_url}
                </code>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  title="Copy — register this exact URL with your provider"
                  onClick={() =>
                    navigator.clipboard?.writeText(data.callback_url!)
                  }
                >
                  <IconCopy className="size-3.5" />
                </Button>
              </div>
            ) : (
              <span className="text-[11px] text-destructive">
                No public URL is set, so there is no redirect URI to register.
                Set one under General first.
              </span>
            )}

            <form
              className="flex flex-col gap-3"
              onSubmit={(e) => {
                e.preventDefault();
                save.mutate(
                  secret ? { ...form, client_secret: secret } : form,
                  {
                    onSuccess: () => setSecret(""),
                  },
                );
              }}
            >
              <Field
                label="Issuer URL"
                help="The base URL. Its /.well-known/openid-configuration is read to find everything else."
              >
                <input
                  className={inputCls}
                  value={form.issuer ?? ""}
                  onChange={(e) => set("issuer", e.target.value)}
                  placeholder="https://login.microsoftonline.com/<tenant>/v2.0"
                />
              </Field>

              <div className="grid gap-3 sm:grid-cols-2">
                <Field label="Client ID">
                  <input
                    className={inputCls}
                    value={form.client_id ?? ""}
                    onChange={(e) => set("client_id", e.target.value)}
                    autoComplete="off"
                  />
                </Field>
                <Field
                  label="Client secret"
                  help={
                    data?.client_secret_set
                      ? "A secret is stored. Leave blank to keep it."
                      : "Not set."
                  }
                >
                  <input
                    className={inputCls}
                    type="password"
                    value={secret}
                    onChange={(e) => setSecret(e.target.value)}
                    autoComplete="new-password"
                  />
                </Field>
              </div>

              <Field
                label="Allowed email domains"
                help="Comma-separated. Leave blank to accept anyone your provider vouches for — right for a single-tenant issuer, wrong for a multi-tenant one."
              >
                <input
                  className={inputCls}
                  value={form.allowed_domains ?? ""}
                  onChange={(e) => set("allowed_domains", e.target.value)}
                  placeholder="example.com, example.org"
                />
              </Field>

              <Field
                label="Button label"
                help="What the sign-in page calls it."
              >
                <input
                  className={inputCls}
                  value={form.label ?? ""}
                  onChange={(e) => set("label", e.target.value)}
                  placeholder="Entra ID"
                />
              </Field>

              <div className="flex items-center gap-2 pt-0.5">
                <Button type="submit" size="sm" disabled={save.isPending}>
                  {save.isPending ? "Saving…" : "Save"}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={!ready || !data?.callback_url || test.isPending}
                  title={
                    ready
                      ? "Sign in once to prove it works — that is what switches it on"
                      : "Save an issuer, client ID and secret first"
                  }
                  onClick={() => test.mutate()}
                >
                  <IconPlugConnected className="size-3.5" />
                  {test.isPending ? "Redirecting…" : "Test and switch on"}
                </Button>
              </div>
            </form>

            {(save.isError || test.isError || config.isError) && (
              <span className="text-[11px] text-destructive">
                {save.error?.message ??
                  test.error?.message ??
                  config.error?.message}
              </span>
            )}
          </CardContent>
        </Card>

        <GithubSso />

        <p className="text-[11px] text-muted-foreground">
          Saving never switches a provider on — a successful test does, and
          changing anything switches it back off. Password sign-in is never
          disabled, so a misconfiguration here costs a retry rather than locking
          everyone out.
        </p>
      </div>
    </SettingsShell>
  );
}
