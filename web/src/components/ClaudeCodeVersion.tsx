import { ArrowUpCircle, Check, Loader2 } from "lucide-react";
import { useClaudeCodeVersion, useUpdateClaudeCode } from "@/lib/system";
import { Button } from "@/components/ui/button";

/**
 * Sidebar-footer widget: shows the installed Claude Code CLI version and, when
 * npm has a newer one, an Update button. The update installs into the
 * container's persistent `$HOME/.local` (see `system_routes.rs`), so it sticks
 * across restarts on a volume-backed home. Renders nothing until the first
 * version check resolves; degrades to version-only when offline.
 */
export function ClaudeCodeVersion() {
  const version = useClaudeCodeVersion();
  const update = useUpdateClaudeCode();
  const info = version.data;

  if (!info) return null;

  const failedMessage = update.isError
    ? update.error.message
    : update.data && !update.data.ok
      ? update.data.message
      : null;

  return (
    <div className="flex flex-col gap-1 px-2 py-1.5 text-[11px] text-muted-foreground">
      <div className="flex items-center gap-1.5">
        <span className="font-medium">Claude Code</span>
        <span className="font-mono">{info.installed ?? "unknown"}</span>
        {info.installed && !info.update_available && !update.isPending && (
          <Check
            className="size-3 text-status-success"
            aria-label="up to date"
          />
        )}
      </div>

      {info.update_available && (
        <div className="flex items-center justify-between gap-2">
          <span className="text-accent-orange">{info.latest} available</span>
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="h-6 gap-1 px-2 text-[11px]"
            disabled={update.isPending}
            onClick={() => update.mutate()}
            title={`Update Claude Code to ${info.latest}`}
          >
            {update.isPending ? (
              <>
                <Loader2 className="size-3 animate-spin" /> Updating…
              </>
            ) : (
              <>
                <ArrowUpCircle className="size-3" /> Update
              </>
            )}
          </Button>
        </div>
      )}

      {failedMessage && (
        <span className="text-status-failed" title={failedMessage}>
          Update failed — see server logs
        </span>
      )}
    </div>
  );
}
