import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { SettingsShell } from "@/components/SettingsShell";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { isAdminOf, useAuthStatus } from "@/lib/auth";
import { useChatSettings, useSetChatSettings } from "@/lib/chat";

/** Base UI's Select needs a concrete value for "nothing chosen". */
const DEFAULT = "__default__";
const EFFORTS = ["low", "medium", "high", "xhigh", "max"];

export function ChatSettingsPage() {
  const settings = useChatSettings();
  const save = useSetChatSettings();
  const admin = isAdminOf(useAuthStatus().data);
  const data = settings.data;

  const [provider, setProvider] = useState(DEFAULT);
  const [model, setModel] = useState(DEFAULT);
  const [effort, setEffort] = useState(DEFAULT);
  useEffect(() => {
    if (!data) return;
    setProvider(data.stored.provider ?? DEFAULT);
    setModel(data.stored.model ?? DEFAULT);
    setEffort(data.stored.effort ?? DEFAULT);
  }, [data]);

  const options = data?.options ?? [];
  // Models belong to the provider: with the default chosen, offer the models
  // of whichever agent the default resolves to.
  const shownProvider = provider === DEFAULT ? data?.provider : provider;
  const models = options.find((o) => o.id === shownProvider)?.models ?? [];

  return (
    <SettingsShell title="Chat">
      <div className="mx-auto flex max-w-3xl flex-col gap-5 p-6">
        <section className="flex flex-col gap-2">
          <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            Agent
          </h2>
          <Card>
            <CardContent className="flex flex-col gap-3 px-4 py-3.5">
              <div>
                <div className="text-[13px] font-medium">
                  What answers in the chat
                </div>
                <div className="text-[11px] text-muted-foreground">
                  The chat's only tools are the harness's own — runs, pull
                  requests, Linear wiring and workflow authoring. It cannot run
                  commands or read files, whichever agent you pick. Changing
                  this applies to new conversations; an open one stays on the
                  agent it started with.
                </div>
              </div>

              {options.length === 0 && settings.isSuccess ? (
                <p className="text-[12px] text-destructive">
                  No agent can run the chat yet. Connect Claude Code, or Pi with
                  a Kimi or ChatGPT login, under{" "}
                  <Link to="/settings/agents" className="underline">
                    Agents
                  </Link>
                  .
                </p>
              ) : (
                <form
                  className="grid gap-3 sm:grid-cols-3"
                  onSubmit={(e) => {
                    e.preventDefault();
                    const v = (s: string) => (s === DEFAULT ? null : s);
                    save.mutate({
                      provider: v(provider),
                      model: v(model),
                      effort: v(effort),
                    });
                  }}
                >
                  <Field label="Agent">
                    <Select
                      value={provider}
                      disabled={!admin}
                      onValueChange={(v) => {
                        if (v == null) return;
                        setProvider(v);
                        // A model is only meaningful for its own agent.
                        setModel(DEFAULT);
                      }}
                    >
                      <SelectTrigger className="w-full">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value={DEFAULT}>
                          Default
                          {data?.provider && !data.stored.provider
                            ? ` (${options.find((o) => o.id === data.provider)?.label ?? data.provider})`
                            : ""}
                        </SelectItem>
                        {options.map((o) => (
                          <SelectItem key={o.id} value={o.id}>
                            {o.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </Field>
                  <Field label="Model">
                    <Select
                      value={model}
                      disabled={!admin}
                      onValueChange={(v) => v != null && setModel(v)}
                    >
                      <SelectTrigger className="w-full">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value={DEFAULT}>
                          Default{models[0] ? ` (${models[0]})` : ""}
                        </SelectItem>
                        {models.map((m) => (
                          <SelectItem key={m} value={m}>
                            {m}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </Field>
                  <Field label="Effort">
                    <Select
                      value={effort}
                      disabled={!admin}
                      onValueChange={(v) => v != null && setEffort(v)}
                    >
                      <SelectTrigger className="w-full">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value={DEFAULT}>Default</SelectItem>
                        {EFFORTS.map((e) => (
                          <SelectItem key={e} value={e}>
                            {e}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </Field>
                  {admin && (
                    <div className="sm:col-span-3">
                      <Button type="submit" size="sm" disabled={save.isPending}>
                        {save.isPending ? "Saving…" : "Save"}
                      </Button>
                    </div>
                  )}
                </form>
              )}

              {!admin && (
                <span className="text-[11px] text-muted-foreground">
                  An administrator chooses the chat's agent.
                </span>
              )}
              {save.isError && (
                <span className="text-[11px] text-destructive">
                  {save.error.message}
                </span>
              )}
              {settings.isError && (
                <span className="text-[11px] text-destructive">
                  {settings.error.message}
                </span>
              )}
            </CardContent>
          </Card>
          <p className="text-[11px] text-muted-foreground">
            Only agents that can be limited to the harness's tools are offered:
            Claude Code and Pi. Starting a run or deleting a workflow from the
            chat always waits for the person to press Confirm.
          </p>
        </section>
      </div>
    </SettingsShell>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1">
      <span className="text-[11px] font-medium text-muted-foreground">
        {label}
      </span>
      {children}
    </div>
  );
}
