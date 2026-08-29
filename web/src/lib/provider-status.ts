/**
 * What a provider row says about itself.
 *
 * Two independent things decide whether a provider can actually run a node: the
 * **credential** (stored here, encrypted) and the **CLI** (baked into the
 * image). A row that reports only the first is misleading — a Cursor credential
 * with no `cursor-agent` on PATH looks connected right up until the run fails.
 * So the two are reported together, and the worse of them wins.
 *
 * A working provider says nothing beyond its dot. Six rows each reading
 * "Connected." under a green dot is the same fact told twice and a page harder
 * to scan for the row that is *not* fine — so `detail` is null when the state
 * speaks for itself, and `label` carries the meaning for anyone who cannot see
 * the dot.
 */
import type { ProviderHealth } from "./system";
import type { ProviderStatus } from "@/components/providers/ProviderMark";

export interface ProviderState {
  status: ProviderStatus;
  /** What the dot means, in words. Always present, never rendered as text. */
  label: string;
  /** Worth saying out loud, or null when the dot is the whole story. */
  detail: string | null;
}

/**
 * Describe one provider.
 *
 * `health` is absent for the providers that are not a CLI at all (GitHub,
 * Linear) and while the probe is still in flight — in both cases the credential
 * is all there is to go on.
 */
export function describeProvider(
  configured: boolean,
  health?: ProviderHealth,
): ProviderState {
  if (health && !health.on_path) {
    return {
      status: "bad",
      label: configured ? "Connected, but the CLI is missing" : "CLI missing",
      detail: configured
        ? `Connected, but \`${health.binary}\` is not installed or not on PATH — nodes using it will fail.`
        : `Not found — \`${health.binary}\` is not installed or not on PATH.`,
    };
  }

  if (!configured) {
    return {
      status: health ? "warn" : "off",
      label: health ? "Installed, no credential" : "Not connected",
      detail: health
        ? `\`${health.binary}\` is installed, but no credential is stored — nodes using it cannot run yet.`
        : "Not connected.",
    };
  }

  if (health?.update_available && health.latest) {
    return {
      status: "ok",
      label: "Connected",
      detail: `Version ${health.latest} is available.`,
    };
  }

  // Working, with nothing to add. The dot says so.
  return { status: "ok", label: "Connected", detail: null };
}
