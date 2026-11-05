/**
 * "Linear account" on the Preferences page: link the Linear account you use at
 * work to the account you sign in here with, so a mention or a delegation from
 * Linear counts as you.
 */
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  identityLabel,
  useLinkLinear,
  useMyIdentities,
  useUnlinkMine,
} from "@/lib/identities";

/**
 * The outcome of the Linear round-trip: the callback sends the browser back
 * here with `?linear_link=…`, which is read once and stripped from the URL.
 */
function useLinkResult() {
  const [result, setResult] = useState<{
    ok: boolean;
    message: string | null;
  } | null>(null);
  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const status = params.get("linear_link");
    if (!status) return;
    setResult({
      ok: status === "linked",
      message: params.get("linear_message"),
    });
    params.delete("linear_link");
    params.delete("linear_message");
    const query = params.toString();
    window.history.replaceState(
      {},
      "",
      `${window.location.pathname}${query ? `?${query}` : ""}`,
    );
  }, []);
  return result;
}

export function LinearAccountSection() {
  const mine = useMyIdentities(true);
  const link = useLinkLinear();
  const unlink = useUnlinkMine();
  const result = useLinkResult();

  const identities = mine.data?.identities ?? [];
  const connections = mine.data?.connections ?? [];
  // Nothing to link through and nothing linked: Linear isn't set up here.
  if (mine.isSuccess && connections.length === 0 && identities.length === 0) {
    return null;
  }

  return (
    <section className="flex flex-col gap-2">
      <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        Linear account
      </h2>
      {result && (
        <div
          role="status"
          className={
            result.ok
              ? "rounded-md border border-status-success/40 bg-status-success/10 px-3 py-2 text-xs"
              : "rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs"
          }
        >
          {result.ok
            ? `Linked to ${result.message ?? "your Linear account"}.`
            : `Linking failed${result.message ? `: ${result.message}` : "."}`}
        </div>
      )}
      <Card>
        <CardContent className="p-0">
          {mine.isPending && (
            <div className="px-4 py-3.5 text-[11px] text-muted-foreground">
              Loading…
            </div>
          )}
          {mine.isError && (
            <div
              role="alert"
              className="px-4 py-3.5 text-[11px] text-destructive"
            >
              {mine.error.message}
            </div>
          )}
          {connections.map((conn) => {
            const linked = identities.find((i) => i.connection === conn.id);
            return (
              <div
                key={conn.id}
                className="flex flex-col gap-3 border-t border-border px-4 py-3.5 first:border-t-0 sm:flex-row sm:items-center sm:justify-between sm:gap-6"
              >
                <div className="min-w-0">
                  <div className="text-[13px] font-medium">
                    {linked ? `Linked as ${identityLabel(linked)}` : conn.label}
                  </div>
                  <div className="text-[11px] text-muted-foreground">
                    {linked
                      ? `In ${conn.label}. Mentions and runs from Linear count as you.`
                      : "Sign in to Linear once so mentions and runs from Linear count as you."}
                  </div>
                </div>
                <div className="shrink-0">
                  {linked ? (
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={unlink.isPending}
                      onClick={() => unlink.mutate({ id: linked.id })}
                    >
                      Unlink
                    </Button>
                  ) : (
                    <Button
                      size="sm"
                      disabled={link.isPending}
                      onClick={() => link.mutate({ connection: conn.id })}
                    >
                      Link Linear account
                    </Button>
                  )}
                </div>
              </div>
            );
          })}
          {/* A link made through a connection that has since gone away. */}
          {identities
            .filter((i) => !connections.some((c) => c.id === i.connection))
            .map((identity) => (
              <div
                key={identity.id}
                className="flex flex-col gap-3 border-t border-border px-4 py-3.5 first:border-t-0 sm:flex-row sm:items-center sm:justify-between sm:gap-6"
              >
                <div className="min-w-0 text-[13px] font-medium">
                  Linked as {identityLabel(identity)}
                </div>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={unlink.isPending}
                  onClick={() => unlink.mutate({ id: identity.id })}
                >
                  Unlink
                </Button>
              </div>
            ))}
          {(link.isError || unlink.isError) && (
            <div
              role="alert"
              className="border-t border-border px-4 py-3 text-[11px] text-destructive"
            >
              {(link.error ?? unlink.error)?.message}
            </div>
          )}
        </CardContent>
      </Card>
    </section>
  );
}
