import { IconBrandOpenai } from "@tabler/icons-react";

/**
 * Brand marks for the providers, so a row is identifiable before it is read.
 *
 * The paths are the official ones from Simple Icons (CC0), copied in rather
 * than pulled as a dependency: six marks do not justify a package of three
 * thousand, and these do not change. OpenAI is the exception -- Simple Icons no
 * longer carries it -- so Codex keeps Tabler's, which is a stroke rosette and
 * authentic to the mark anyway.
 *
 * Source: simple-icons 16.29.0.
 */

/** Filled 24x24 marks, drawn in one path each. */
const PATHS: Record<string, string> = {
  claude:
    "m4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z",
  cursor:
    "M11.503.131 1.891 5.678a.84.84 0 0 0-.42.726v11.188c0 .3.162.575.42.724l9.609 5.55a1 1 0 0 0 .998 0l9.61-5.55a.84.84 0 0 0 .42-.724V6.404a.84.84 0 0 0-.42-.726L12.497.131a1.01 1.01 0 0 0-.996 0M2.657 6.338h18.55c.263 0 .43.287.297.515L12.23 22.918c-.062.107-.229.064-.229-.06V12.335a.59.59 0 0 0-.295-.51l-9.11-5.257c-.109-.063-.064-.23.061-.23",
  github:
    "M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12",
  kimi: "M21.765.351C22.998.351 24 1.353 24 2.586S22.998 4.82 21.765 4.82h-1.974c-.15 0-.26-.12-.26-.26V2.586A2.237 2.237 0 0 1 21.765.35M9.41 13.388l8.447-8.377c.16-.16.07-.471-.14-.471h-4.55s-.1.02-.14.06l-9.099 9.029c-.14.14-.35.02-.35-.21V4.81c0-.15-.1-.27-.221-.27H.22c-.12 0-.22.12-.22.27v18.57c0 .15.1.27.22.27h3.137c.12 0 .22-.12.22-.27v-3.79c0-.08.03-.16.08-.21l2.826-2.796c.07-.07.16-.08.241-.03l7.546 5.551a8.9 8.9 0 0 0 4.018 1.493c.12.01.23-.11.23-.27V19.76c0-.14-.08-.25-.19-.26a5.8 5.8 0 0 1-2.355-.942l-6.533-4.73c-.14-.09-.15-.32-.03-.441",
  linear:
    "M2.886 4.18A11.982 11.982 0 0 1 11.99 0C18.624 0 24 5.376 24 12.009c0 3.64-1.62 6.903-4.18 9.105L2.887 4.18ZM1.817 5.626l16.556 16.556c-.524.33-1.075.62-1.65.866L.951 7.277c.247-.575.537-1.126.866-1.65ZM.322 9.163l14.515 14.515c-.71.172-1.443.282-2.195.322L0 11.358a12 12 0 0 1 .322-2.195Zm-.17 4.862 9.823 9.824a12.02 12.02 0 0 1-9.824-9.824Z",
};

/**
 * Brand colour, but only where it survives both themes.
 *
 * GitHub, Cursor and Kimi are officially black or near it, which is invisible
 * on a dark ground -- those inherit the text colour instead, which is what
 * their monochrome marks are designed for.
 */
const TINT: Record<string, string> = {
  claude: "text-[#D97757]",
  linear: "text-[#5E6AD2]",
};

/**
 * Marks we have no path for, drawn as a letter instead.
 *
 * Pi ships no mark in Simple Icons, and inventing one would be worse than a
 * plain glyph: a made-up logo reads as authoritative and is simply wrong. This
 * is honest about being a stand-in, and can be replaced the day the real mark
 * is to hand.
 */
const GLYPHS: Record<string, string> = { pi: "π" };

export type ProviderStatus = "ok" | "warn" | "bad" | "off";

const DOT: Record<ProviderStatus, string> = {
  ok: "bg-status-success",
  warn: "bg-accent-orange",
  bad: "bg-status-failed",
  off: "bg-muted-foreground/40",
};

/**
 * A provider's mark with its state as a dot on the corner.
 *
 * `label` is what the dot means in words. It is not decoration: the row only
 * spells the state out underneath when there is something to add, so for a
 * provider that is simply working the dot is the whole message -- and a colour
 * alone is no message at all to a screen reader, or to someone who cannot tell
 * these two greens and reds apart.
 */
export function ProviderMark({
  provider,
  status,
  label,
}: {
  /**
   * Which **brand** to draw — not always the credential key. `pi` is the Pi
   * agent; the subscription behind it is `kimi`, and drawing Kimi's mark on the
   * agent would say the agent is Kimi's. It is not: Pi runs whatever model its
   * namespace selects, on either of two accounts.
   */
  provider: string;
  status: ProviderStatus;
  label: string;
}) {
  const path = PATHS[provider];
  return (
    <span
      role="img"
      aria-label={label}
      title={label}
      className="relative inline-flex shrink-0 items-center justify-center"
    >
      <span className={TINT[provider] ?? "text-foreground"}>
        {path ? (
          <svg className="size-5" viewBox="0 0 24 24" fill="currentColor">
            <path d={path} />
          </svg>
        ) : provider === "codex" ? (
          <IconBrandOpenai className="size-5" />
        ) : GLYPHS[provider] ? (
          <span className="flex size-5 items-center justify-center font-serif text-[17px] leading-none">
            {GLYPHS[provider]}
          </span>
        ) : (
          <span className="block size-5" />
        )}
      </span>
      <span
        aria-hidden="true"
        className={`absolute -left-1 -top-1 size-2 rounded-full ring-2 ring-card ${DOT[status]}`}
      />
    </span>
  );
}
