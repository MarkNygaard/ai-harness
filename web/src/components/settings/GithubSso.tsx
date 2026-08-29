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
import type { GithubAudience, GithubSsoInput } from "@/lib/sso";

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

/** The two ways to bound who may sign in. There is deliberately no third. */
const AUDIENCES: {
  value: GithubAudience;
  title: string;
  help: string;
}[] = [
  {
    value: "org",
    title: "Members of an organisation",
    help: "Anyone in the GitHub organisation below, optionally narrowed to one team. First sign-in creates their account.",
  },
  {
    value: "existing",
    title: "People who already have an account here",
    help: "Matched on the verified email GitHub holds for them. No account is ever created, so the allowlist is exactly who you have invited.",
  },
];

/**
 * GitHub sign-in.
 *
 * Unlike the OIDC provider's email-domain list, which may be left blank for a
 * single-tenant issuer, this always needs a boundary: a GitHub account is free
 * and anyone can have one, so the provider itself is not one. Which boundary
 * is the choice below.
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
      audience: data.audience ?? "org",
      org: data.org ?? "",
      team: data.team ?? "",
    });
    setSeeded(true);
  }, [data, seeded]);

  const set = <K extends keyof GithubSsoInput>(
    key: K,
    value: GithubSsoInput[K],
  ) => setForm((f) => ({ ...f, [key]: value }));

  const audience: GithubAudience = form.audience ?? data?.audience ?? "org";
  // Existing-accounts mode carries its allowlist in the user table, so there is
  // nothing further to fill in.
  const ready =
    !!data?.client_id &&
    data?.client_secret_set &&
    (data?.audience === "existing" || !!data?.org?.trim());

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

            <fieldset className="flex flex-col gap-1.5">
              <legend className="mb-1 text-[11px] font-medium text-muted-foreground">
                Who may sign in
              </legend>
              {AUDIENCES.map((option) => (
                <label
                  key={option.value}
                  className="flex cursor-pointer items-start gap-2"
                >
                  <input
                    type="radio"
                    name="github-audience"
                    className="mt-0.5 size-3.5 shrink-0"
                    checked={audience === option.value}
                    onChange={() => set("audience", option.value)}
                  />
                  <span className="flex flex-col gap-0.5">
                    <span className="text-[13px] leading-tight">
                      {option.title}
                    </span>
                    <span className="text-[10px] leading-snug text-muted-foreground">
                      {option.help}
                    </span>
                  </span>
                </label>
              ))}
            </fieldset>

            {audience === "org" && (
              <div className="grid gap-3 sm:grid-cols-2">
                <Field
                  label="Organisation"
                  help="Required for this mode — a GitHub account alone is not enough, since anyone can have one."
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
            )}

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
                    : "Save a client ID and secret first, and an organisation if you chose that mode"
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
