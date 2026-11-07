import { Link } from "react-router-dom";
import { IconEye, IconEyeOff } from "@tabler/icons-react";
import { SettingsShell } from "@/components/SettingsShell";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useOperationsNav } from "@/components/AppSidebar";
import { LinearAccountSection } from "@/components/settings/LinearAccountSection";
import { useAuthStatus } from "@/lib/auth";
import { resetNavHidden, toggleNavHidden, useHiddenNav } from "@/lib/nav-prefs";
import {
  type LinearTriggerHome,
  setLinearTriggerHome,
  useLinearTriggerHome,
} from "@/lib/trigger-prefs";
import { useTheme } from "@/lib/theme";
import type { ThemePreference } from "@/lib/theme";

/**
 * One labelled setting: a title, a line saying what it does, and the control.
 *
 * Settings read as a scanned list rather than a form, so the description sits
 * with the label and the control is pushed right — you can find the row you
 * want without reading every control on the page.
 */
function Row({
  title,
  description,
  children,
}: {
  title: string;
  description: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-3 border-t border-border px-4 py-3.5 first:border-t-0 sm:flex-row sm:items-center sm:justify-between sm:gap-6">
      <div className="min-w-0">
        <div className="text-[13px] font-medium">{title}</div>
        <div className="text-[11px] text-muted-foreground">{description}</div>
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="flex flex-col gap-2">
      <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        {title}
      </h2>
      <Card>
        <CardContent className="p-0">{children}</CardContent>
      </Card>
    </section>
  );
}

const THEMES: { value: ThemePreference; label: string }[] = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

/** Segmented control — a few options, all worth seeing at once. */
function Segmented<T extends string>({
  options,
  value,
  onChange,
}: {
  options: { value: T; label: string }[];
  value: T;
  onChange: (value: T) => void;
}) {
  return (
    <div className="flex rounded-md border border-input p-0.5">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          onClick={() => onChange(o.value)}
          aria-pressed={value === o.value}
          className={
            value === o.value
              ? "rounded-sm bg-secondary px-3 py-1 text-[12px] font-medium"
              : "rounded-sm px-3 py-1 text-[12px] text-muted-foreground hover:text-foreground"
          }
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

function ThemeChoice() {
  const { preference, setPreference } = useTheme();
  return (
    <Segmented options={THEMES} value={preference} onChange={setPreference} />
  );
}

const LINEAR_HOMES: { value: LinearTriggerHome; label: string }[] = [
  { value: "triggers", label: "Triggers page" },
  { value: "project", label: "Project tab" },
];

function LinearTriggerHomeChoice() {
  const home = useLinearTriggerHome();
  return (
    <Segmented
      options={LINEAR_HOMES}
      value={home}
      onChange={setLinearTriggerHome}
    />
  );
}

/**
 * Show or hide entries in the app sidebar.
 *
 * This is the old "Edit menu" mode, moved out of the sidebar itself: editing a
 * menu from inside that menu meant a mode toggle sitting permanently in the
 * navigation, for something most people set once.
 */
function SidebarItems() {
  const items = useOperationsNav();
  const { hidden, isHidden } = useHiddenNav();

  return (
    <>
      {items.map((item) => (
        <Row key={item.href} title={item.label} description={item.href}>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => toggleNavHidden(item.href)}
            aria-pressed={isHidden(item.href)}
            title={
              isHidden(item.href) ? "Show in sidebar" : "Hide from sidebar"
            }
          >
            {isHidden(item.href) ? (
              <>
                <IconEyeOff className="size-3.5" /> Hidden
              </>
            ) : (
              <>
                <IconEye className="size-3.5" /> Shown
              </>
            )}
          </Button>
        </Row>
      ))}
      {hidden.length > 0 && (
        <Row
          title="Show everything again"
          description={`${hidden.length} ${hidden.length === 1 ? "entry is" : "entries are"} hidden.`}
        >
          <Button variant="outline" size="sm" onClick={resetNavHidden}>
            Reset
          </Button>
        </Row>
      )}
    </>
  );
}

export function PreferencesPage() {
  const auth = useAuthStatus();
  const signedIn = auth.data?.mode === "accounts" && !!auth.data.user;
  return (
    <SettingsShell title="Preferences">
      <div className="mx-auto flex max-w-3xl flex-col gap-6 p-6">
        {/* Belongs to your account, not this browser — so it sits above the
            line that says the rest is browser-only. */}
        {signedIn && <LinearAccountSection />}

        <p className="max-w-prose text-xs text-muted-foreground">
          {signedIn ? "The settings below are personal" : "Personal"} to this
          browser. Nothing here is shared with anyone else using this harness.
        </p>

        <Section title="Interface and theme">
          <Row
            title="Theme"
            description="System follows your operating system, and keeps following it as it changes."
          >
            <ThemeChoice />
          </Row>
        </Section>

        <Section title="Triggers">
          <Row
            title="Manage Linear triggers on"
            description="The Triggers page lists them with every other kind. The project tab is the older home, on each project's own page. Both edit the same triggers."
          >
            <LinearTriggerHomeChoice />
          </Row>
        </Section>

        <Section title="App sidebar">
          <SidebarItems />
        </Section>

        <p className="text-[11px] text-muted-foreground">
          Report pages declared by a workflow appear in the sidebar once that
          workflow has a run, and can be hidden here too. Manage them in{" "}
          <Link className="underline" to="/settings/workflows">
            Workflows
          </Link>
          .
        </p>
      </div>
    </SettingsShell>
  );
}
