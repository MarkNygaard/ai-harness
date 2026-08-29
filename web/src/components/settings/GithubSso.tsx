import { useEffect, useState } from "react";
import { IconBrandGithub, IconCopy } from "@tabler/icons-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  useGithubSsoConfig,
  useSaveGithubSso,
  useTestGithubSso,
} from "@/lib/sso";
import type { GithubSsoInput } from "@/lib/sso";

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

/**
 * GitHub sign-in.
 *
 * An organisation is required rather than optional, unlike the OIDC provider's
 * email-domain list: a GitHub account is free and anyone can have one, so
 * without an organisation the allowlist would be everybody.
 */
export function GithubSso() {
  const config = useGithubSsoConfig(true);
  const save = useSaveGithubSso();
  const test = useTestGithubSso();

  const [form, setForm] = useState<GithubSsoInput>({});
  const [secret, setSecret] = useState("");
  const [seeded, setSeeded] = useState(false);
  const data = config.data;

  useEffect(() => {
    if (!data || seeded) return;
    setForm({
      client_id: data.client_id ?? "",
      org: data.org ?? "",
      team: data.team ?? "",
    });
    setSeeded(true);
  }, [data, seeded]);

  const set = <K extends keyof GithubSsoInput>(
    key: K,
    value: GithubSsoInput[K],
  ) => setForm((f) => ({ ...f, [key]: value }));

  const ready =
    !!data?.client_id && data?.client_secret_set && !!data?.org?.trim();

  return (
    <section className="flex flex-col gap-2">
      <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        GitHub
      </h2>

      <Card>
        <CardContent className="flex flex-col gap-3 px-4 py-3.5">
          <div className="flex items-center gap-2">
            <IconBrandGithub className="size-4 text-muted-foreground" />
            <span className="text-[13px] font-medium">GitHub</span>
            <Badge
              variant={data?.enabled ? "success" : "outline"}
              className="text-[10px]"
            >
              {data?.enabled ? "in use" : "not switched on"}
            </Badge>
          </div>

          {data?.callback_url ? (
            <div className="flex items-center gap-2">
              <span className="w-24 shrink-0 text-[10px] text-muted-foreground">
                Callback URL
              </span>
              <code className="min-w-0 flex-1 truncate rounded bg-muted px-1.5 py-0.5 font-mono text-[10px]">
                {data.callback_url}
              </code>
              <Button
                variant="ghost"
                size="icon-sm"
                title="Copy — register this exact URL on the GitHub OAuth app"
                onClick={() =>
                  navigator.clipboard?.writeText(data.callback_url!)
                }
              >
                <IconCopy className="size-3.5" />
              </Button>
            </div>
          ) : (
            <span className="text-[11px] text-destructive">
              No public URL is set, so there is no callback URL to register. Set
              one under General first.
            </span>
          )}

          <form
            className="flex flex-col gap-3"
            onSubmit={(e) => {
              e.preventDefault();
              save.mutate(secret ? { ...form, client_secret: secret } : form, {
                onSuccess: () => setSecret(""),
              });
            }}
          >
            <div className="grid gap-3 sm:grid-cols-2">
              <Field
                label="Client ID"
                help="GitHub → Settings → Developer settings → OAuth Apps."
              >
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

            <div className="grid gap-3 sm:grid-cols-2">
              <Field
                label="Organisation"
                help="Required. Members of this org may sign in — a GitHub account alone is not enough, since anyone can have one."
              >
                <input
                  className={inputCls}
                  value={form.org ?? ""}
                  onChange={(e) => set("org", e.target.value)}
                  placeholder="your-org"
                />
              </Field>
              <Field
                label="Team (optional)"
                help="Narrow it further, by team slug."
              >
                <input
                  className={inputCls}
                  value={form.team ?? ""}
                  onChange={(e) => set("team", e.target.value)}
                  placeholder="engineering"
                />
              </Field>
            </div>

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
                    : "Save a client ID, secret and organisation first"
                }
                onClick={() => test.mutate()}
              >
                <IconBrandGithub className="size-3.5" />
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
    </section>
  );
}
