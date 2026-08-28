import { AppShell } from "@/components/AppShell";
import { SettingsSidebar } from "@/components/SettingsSidebar";

/**
 * The settings frame: the application shell with its nav replaced.
 *
 * A thin wrapper rather than a second layout, so every settings section gets
 * the same header, scroll behaviour and `viewActions` handling the rest of the
 * app already has.
 */
export function SettingsShell(
  props: Omit<React.ComponentProps<typeof AppShell>, "sidebar">,
) {
  return <AppShell {...props} sidebar={<SettingsSidebar variant="inset" />} />;
}
