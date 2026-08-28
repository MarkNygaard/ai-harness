import { useState } from "react";
import { IconPlus, IconTrash } from "@tabler/icons-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useCreateToken, useRevokeToken, useTokens } from "@/lib/tokens";

function whenever(iso: string | null): string {
  if (!iso) return "never used";
  const days = Math.floor((Date.now() - Date.parse(iso)) / 86_400_000);
  if (days <= 0) return "used today";
  if (days === 1) return "used yesterday";
  if (days < 60) return `used ${days} days ago`;
  return `last used ${new Date(iso).toLocaleDateString()}`;
}

/**
 * Personal access tokens, and the one moment a token's value exists outside
 * the program that will use it.
 *
 * `onMinted` hands the fresh value up so the connection snippet can render
 * fully populated while it is still on screen — after that the server only has
 * a hash, and there is nothing to show again.
 */
export function PersonalTokens({
  onMinted,
}: {
  onMinted: (secret: string) => void;
}) {
  const tokens = useTokens(true);
  const create = useCreateToken();
  const revoke = useRevokeToken();
  const [name, setName] = useState("");
  const [justMinted, setJustMinted] = useState<string | null>(null);

  const list = tokens.data ?? [];
  const error = tokens.error ?? create.error ?? revoke.error;

  return (
    <section className="flex flex-col gap-2">
      <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        Your tokens
      </h2>

      <Card>
        <CardContent className="p-0">
          {list.map((t) => (
            <div
              key={t.id}
              className="flex items-center gap-2 border-t border-border px-4 py-2.5 first:border-t-0"
            >
              <div className="min-w-0 flex-1">
                <div className="truncate text-[13px] font-medium">{t.name}</div>
                <div className="text-[11px] text-muted-foreground">
                  {whenever(t.last_used_at)}
                </div>
              </div>
              <Button
                variant="ghost"
                size="sm"
                className="text-muted-foreground"
                disabled={revoke.isPending}
                title="Revoke this token — anything using it stops working"
                onClick={() => {
                  if (
                    window.confirm(
                      `Revoke “${t.name}”? Anything configured with it stops working immediately.`,
                    )
                  ) {
                    revoke.mutate(t.id);
                  }
                }}
              >
                <IconTrash className="size-3.5" />
                Revoke
              </Button>
            </div>
          ))}

          {list.length === 0 && !tokens.isLoading && (
            <p className="px-4 py-3 text-[11px] text-muted-foreground">
              No tokens yet. Create one below and the snippet above will carry
              it.
            </p>
          )}

          <form
            className="flex items-center gap-2 border-t border-border px-4 py-2.5"
            onSubmit={(e) => {
              e.preventDefault();
              const trimmed = name.trim();
              if (!trimmed) return;
              create.mutate(
                { name: trimmed },
                {
                  onSuccess: (data) => {
                    setName("");
                    setJustMinted(data.secret);
                    onMinted(data.secret);
                  },
                },
              );
            }}
          >
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="What is it for? e.g. laptop, CI"
              aria-label="Name for the new token"
              className="h-8 min-w-0 flex-1 rounded-md border border-input bg-transparent px-2 text-[13px] outline-none focus:ring-2 focus:ring-ring"
            />
            <Button
              type="submit"
              size="sm"
              variant="outline"
              disabled={!name.trim() || create.isPending}
            >
              <IconPlus className="size-3.5" />
              {create.isPending ? "Creating…" : "Create token"}
            </Button>
          </form>
        </CardContent>
      </Card>

      {justMinted && (
        <p className="text-[11px] text-status-running">
          Your new token is in the snippet above. Copy it now — the harness
          keeps only a hash, so it cannot be shown again.
        </p>
      )}

      {error && <p className="text-[11px] text-destructive">{error.message}</p>}
    </section>
  );
}
