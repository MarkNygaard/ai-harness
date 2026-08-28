import { IconDeviceDesktop, IconMoon, IconSun } from "@tabler/icons-react";
import { Button } from "@/components/ui/button";
import { useTheme } from "@/lib/theme";
import type { ThemePreference } from "@/lib/theme";

/** System first, so the cycle starts from the default and returns to it. */
const CYCLE: ThemePreference[] = ["system", "light", "dark"];

const ICON = {
  system: IconDeviceDesktop,
  light: IconSun,
  dark: IconMoon,
} as const;

const LABEL = {
  system: "Matching your system",
  light: "Light",
  dark: "Dark",
} as const;

/**
 * Cycle through system → light → dark.
 *
 * A cycling button rather than a menu: there are only three states, and the
 * icon already says which one is active — a dropdown would be two clicks to say
 * the same thing.
 */
export function ThemeToggle() {
  const { preference, setPreference } = useTheme();
  const Icon = ICON[preference];
  const next = CYCLE[(CYCLE.indexOf(preference) + 1) % CYCLE.length];

  return (
    <Button
      variant="ghost"
      size="icon-sm"
      onClick={() => setPreference(next)}
      aria-label={`Theme: ${LABEL[preference].toLowerCase()}. Switch to ${LABEL[next].toLowerCase()}.`}
      title={`Theme: ${LABEL[preference]} — click for ${LABEL[next].toLowerCase()}`}
    >
      <Icon className="size-4" />
    </Button>
  );
}
