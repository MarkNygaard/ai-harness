/**
 * What a provider row says about itself.
 *
 * Two independent things decide whether a provider can actually run a node: the
 * **credential** (stored here, encrypted) and the **CLI** (baked into the
 * image). A row that reports only the first is misleading — a Cursor credential
 * with no `cursor-agent` on PATH looks connected right up until the run fails.
 * So the two are reported together, and the worse of them wins.
 */
import type { ProviderHealth } from "./system";
import type { ProviderStatus } from "@/components/providers/ProviderMark";

export interface ProviderState {
  status: ProviderStatus;
  /** One sentence, always present — the dot is never the only signal. */
  detail: string;
}

/**
 * Describe one provider.
 *
 * `health` is absent for the providers that are not a CLI at all (GitHub,
 * Linear) and while the probe is still in flight — in both cases the credential
 * is all there is to go on, which is what the page said before.
 */
export function describeProvider(
  configured: boolean,
  health?: ProviderHealth,
): ProviderState {
  if (health && !health.on_path) {
    return {
      status: "bad",
      detail: configured
        ? `Connected, but \`${health.binary}\` is not installed or not on PATH — nodes using it will fail.`
        : `Not found — \`${health.binary}\` is not installed or not on PATH.`,
    };
  }

  if (!configured) {
    return {
      status: health ? "warn" : "off",
      detail: health
        ? `\`${health.binary}\` is installed, but no credential is stored — nodes using it cannot run yet.`
        : "Not connected.",
    };
  }

  if (health?.update_available && health.latest) {
    return {
      status: "ok",
      detail: `Connected. Version ${health.latest} is available.`,
    };
  }

  return { status: "ok", detail: "Connected." };
}
