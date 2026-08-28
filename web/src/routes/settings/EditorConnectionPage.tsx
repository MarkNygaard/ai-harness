import { useState } from "react";
import { Link } from "react-router-dom";
import {
  IconCopy,
  IconEye,
  IconEyeOff,
  IconRefresh,
} from "@tabler/icons-react";
import { PersonalTokens } from "@/components/settings/PersonalTokens";
import { SettingsShell } from "@/components/SettingsShell";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  MCP_CLIENTS,
  mcpSnippet,
  useMcpConnection,
  useRegenerateMcpKey,
} from "@/lib/mcp";
import type { McpClient } from "@/lib/mcp";
import { useAuthStatus } from "@/lib/auth";

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="flex flex-col gap-2">
      <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        {title}
      </h2>
      {children}
    </section>
  );
}

function CopyButton({ value, label }: { value: string; label: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Button
      variant="ghost"
      size="sm"
      title={label}
      onClick={() => {
        navigator.clipboard?.writeText(value);
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1500);
      }}
    >
      <IconCopy className="size-3.5" />
      {copied ? "Copied" : "Copy"}
    </Button>
  );
}

/** The key, hidden until asked for — a screenshot of this page shouldn't leak it. */
function KeyRow({ token }: { token: string }) {
  const [shown, setShown] = useState(false);
  return (
    <div className="flex items-center gap-2 px-4 py-3">
      <code className="min-w-0 flex-1 truncate rounded bg-muted px-2 py-1 font-mono text-[11px]">
        {shown ? token : "•".repeat(32)}
      </code>
      <Button
        variant="ghost"
        size="sm"
        onClick={() => setShown((v) => !v)}
        title={shown ? "Hide" : "Reveal"}
      >
        {shown ? (
          <IconEyeOff className="size-3.5" />
        ) : (
          <IconEye className="size-3.5" />
        )}
        {shown ? "Hide" : "Reveal"}
      </Button>
      <CopyButton value={token} label="Copy the MCP key" />
    </div>
  );
}

export function EditorConnectionPage() {
  const connection = useMcpConnection();
  const regenerate = useRegenerateMcpKey();
  const [client, setClient] = useState<McpClient>("claude-code");

  const status = useAuthStatus();
  const hasAccounts = status.data?.mode === "accounts";
  // A token exists in the clear for exactly as long as this page stays open.
  const [minted, setMinted] = useState<string | null>(null);

  const data = connection.data;
  const endpoint = data?.endpoint ?? null;
  // With accounts, the snippet should carry *your* token rather than the shared
  // key — a run it starts is then attributable to you. Until one is minted there
  // is nothing to show, so it falls back to a placeholder.
  const secret = hasAccounts ? minted : (data?.token ?? null);
  const snippet = endpoint ? mcpSnippet(client, endpoint, secret) : null;

  return (
    <SettingsShell title="Editor connection">
      <div className="mx-auto flex max-w-3xl flex-col gap-5 p-6">
        <p className="max-w-prose text-xs text-muted-foreground">
          Drive this harness from your editor over MCP — trigger runs, check
          their status, and author workflows without leaving the project you are
          working in.
        </p>

        {connection.isError && (
          <p className="text-xs text-destructive">{connection.error.message}</p>
        )}

        {!endpoint && !connection.isLoading && (
          <Card>
            <CardContent className="px-4 py-3 text-xs">
              No public URL is configured, so there is no address to give your
              editor. Set <code className="font-mono">HARNESS_PUBLIC_URL</code>{" "}
              (or <code className="font-mono">server.public_url</code>) and this
              page will render a snippet you can paste.
            </CardContent>
          </Card>
        )}

        {data?.unauthenticated && (
          <Card>
            <CardContent className="px-4 py-3 text-xs text-destructive">
              <strong>This endpoint is unauthenticated.</strong> The harness has
              nowhere to keep an MCP key — it needs a database and{" "}
              <code className="font-mono">HARNESS_SECRET_KEY</code> — so anyone
              who can reach <code className="font-mono">/mcp</code> can trigger
              runs and edit workflows.
            </CardContent>
          </Card>
        )}

        {endpoint && (
          <>
            <Section title="Endpoint">
              <Card>
                <CardContent className="flex items-center gap-2 px-4 py-3">
                  <code className="min-w-0 flex-1 truncate rounded bg-muted px-2 py-1 font-mono text-[11px]">
                    {endpoint}
                  </code>
                  <CopyButton value={endpoint} label="Copy the endpoint URL" />
                </CardContent>
              </Card>
            </Section>

            {data?.token && !hasAccounts && (
              <Section title="Key">
                <Card>
                  <CardContent className="p-0">
                    <KeyRow token={data.token} />
                    <div className="flex flex-col gap-2 border-t border-border px-4 py-3 sm:flex-row sm:items-center sm:justify-between">
                      <span className="text-[11px] text-muted-foreground">
                        Generated by the harness and stored encrypted. Replacing
                        it stops every editor already configured with it.
                      </span>
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={regenerate.isPending}
                        onClick={() => {
                          if (
                            window.confirm(
                              "Replace the MCP key? Every editor configured with the current one will stop working until you paste the new snippet.",
                            )
                          ) {
                            regenerate.mutate();
                          }
                        }}
                      >
                        <IconRefresh className="size-3.5" />
                        {regenerate.isPending ? "Replacing…" : "Regenerate"}
                      </Button>
                    </div>
                    {regenerate.isError && (
                      <p className="border-t border-border px-4 py-2 text-[11px] text-destructive">
                        {regenerate.error.message}
                      </p>
                    )}
                  </CardContent>
                </Card>
              </Section>
            )}

            <Section title="Add it to your editor">
              <Card>
                <CardContent className="p-0">
                  <div className="flex flex-wrap gap-1 border-b border-border p-2">
                    {MCP_CLIENTS.map((c) => (
                      <button
                        key={c.id}
                        type="button"
                        onClick={() => setClient(c.id)}
                        aria-pressed={client === c.id}
                        className={
                          client === c.id
                            ? "rounded-sm bg-secondary px-2.5 py-1 text-[12px] font-medium"
                            : "rounded-sm px-2.5 py-1 text-[12px] text-muted-foreground hover:text-foreground"
                        }
                      >
                        {c.label}
                      </button>
                    ))}
                  </div>
                  <div className="flex items-center justify-between gap-2 px-4 py-2">
                    <span className="font-mono text-[11px] text-muted-foreground">
                      {MCP_CLIENTS.find((c) => c.id === client)?.file}
                    </span>
                    {snippet && (
                      <CopyButton
                        value={snippet}
                        label="Copy the configuration"
                      />
                    )}
                  </div>
                  <pre className="overflow-x-auto border-t border-border px-4 py-3 font-mono text-[11px] leading-relaxed">
                    {snippet}
                  </pre>
                </CardContent>
              </Card>
              <p className="text-[11px] text-muted-foreground">
                This snippet contains a secret. Keep it out of a project&rsquo;s{" "}
                <code className="font-mono">.mcp.json</code>, which is the file
                that gets committed — the Claude Code command above writes to
                your user configuration instead.
              </p>
            </Section>

            {hasAccounts && <PersonalTokens onMinted={setMinted} />}

            <Section title="What your editor gets">
              <Card>
                <CardContent className="flex flex-col gap-3 px-4 py-3 text-[11px]">
                  <div>
                    <div className="mb-1 font-medium">Runs</div>
                    <div className="font-mono text-muted-foreground">
                      {data?.run_tools.join(" · ")}
                    </div>
                  </div>
                  <div>
                    <div className="mb-1 font-medium">Authoring</div>
                    <div className="font-mono text-muted-foreground">
                      {data?.authoring_tools.join(" · ")}
                    </div>
                  </div>
                </CardContent>
              </Card>
            </Section>
          </>
        )}

        <p className="text-[11px] text-muted-foreground">
          Workflows live in{" "}
          <Link className="underline" to="/settings/workflows">
            Workflows
          </Link>
          , and the projects runs operate on are in{" "}
          <Link className="underline" to="/settings/projects">
            Projects
          </Link>
          .
        </p>
      </div>
    </SettingsShell>
  );
}
