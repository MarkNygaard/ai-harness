import { useEffect, useState } from "react";
import { SettingsShell } from "@/components/SettingsShell";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useGeneralSettings, useSetGeneralSettings } from "@/lib/settings";

export function GeneralPage() {
  const settings = useGeneralSettings(true);
  const save = useSetGeneralSettings();
  const [value, setValue] = useState("");
  const [touched, setTouched] = useState(false);

  const data = settings.data;
  // Seed the field once, then leave the operator's typing alone.
  useEffect(() => {
    if (data && !touched) setValue(data.public_url ?? "");
  }, [data, touched]);

  const inherited = !data?.stored && !!data?.from_environment;

  return (
    <SettingsShell title="General">
      <div className="mx-auto flex max-w-3xl flex-col gap-5 p-6">
        <section className="flex flex-col gap-2">
          <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            Domain
          </h2>
          <Card>
            <CardContent className="flex flex-col gap-3 px-4 py-3.5">
              <div>
                <div className="text-[13px] font-medium">Public URL</div>
                <div className="text-[11px] text-muted-foreground">
                  The address this harness tells the outside world to use. It
                  builds the Linear callback and webhook URLs, the MCP endpoint,
                  and every run link.
                </div>
              </div>

              <form
                className="flex flex-col gap-2 sm:flex-row sm:items-center"
                onSubmit={(e) => {
                  e.preventDefault();
                  save.mutate({ public_url: value.trim() || null });
                }}
              >
                <input
                  value={value}
                  onChange={(e) => {
                    setValue(e.target.value);
                    setTouched(true);
                  }}
                  placeholder="https://harness.example.com"
                  aria-label="Public URL"
                  className="h-8 min-w-0 flex-1 rounded-md border border-input bg-transparent px-2 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
                />
                <Button type="submit" size="sm" disabled={save.isPending}>
                  {save.isPending ? "Saving…" : "Save"}
                </Button>
                {data?.stored && (
                  <Button
                    type="button"
                    size="sm"
                    variant="ghost"
                    disabled={save.isPending}
                    title="Go back to the value from the environment"
                    onClick={() => {
                      setTouched(false);
                      save.mutate({ public_url: null });
                    }}
                  >
                    Clear
                  </Button>
                )}
              </form>

              {inherited && (
                <span className="text-[11px] text-muted-foreground">
                  Currently inherited from{" "}
                  <code className="font-mono">HARNESS_PUBLIC_URL</code>. Saving
                  here overrides it without a redeploy.
                </span>
              )}
              {!data?.public_url && !settings.isLoading && (
                <span className="text-[11px] text-destructive">
                  Not set. Connecting Linear and connecting an editor both need
                  an address to hand out.
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
            This only sets the address the harness advertises. It does not
            provision DNS or a certificate — those stay with your ingress or
            tunnel.
          </p>
        </section>
      </div>
    </SettingsShell>
  );
}
