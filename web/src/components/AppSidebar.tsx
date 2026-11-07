import { Link, useLocation } from "react-router-dom";
import {
  IconBolt,
  IconClipboardCheck,
  IconGitCompare,
  IconLayoutDashboard,
  IconMessageCircle,
  IconReportSearch,
  IconRocket,
  IconSearch,
  IconShieldCheck,
  IconWorldSearch,
  IconZoomCode,
} from "@tabler/icons-react";
import { Factory } from "lucide-react";
import { useHiddenNav } from "@/lib/nav-prefs";
import { useRuns } from "@/lib/runs";
import { useWorkflowList } from "@/lib/authoring";
import { AccountMenu } from "@/components/AccountMenu";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/components/ui/sidebar";

/**
 * Which part of the sidebar an entry belongs to: the harness's own tools, the
 * same on every install, or a report page a workflow declared (`ui.nav`).
 */
export type NavGroup = "harness" | "reports";

/** The sidebar's groups, in order, with the heading each shows (if any). */
export const NAV_GROUPS: { group: NavGroup; label: string | null }[] = [
  // The tools need no heading: they are what the app *is*.
  { group: "harness", label: null },
  { group: "reports", label: "Reports" },
];

export interface NavItem {
  href: string;
  label: string;
  icon: typeof IconRocket;
  /** Active when the path equals href, or (for prefixes) starts with `match`. */
  match?: string;
  group: NavGroup;
}

/** Curated icon allow-list for workflow-declared nav entries (`ui.nav.icon`),
 * so YAML picks an icon by key rather than injecting a component. */
const NAV_ICONS: Record<string, typeof IconRocket> = {
  shield: IconShieldCheck,
  "world-search": IconWorldSearch,
  "zoom-code": IconZoomCode,
  search: IconSearch,
  report: IconReportSearch,
  checklist: IconClipboardCheck,
};
const DEFAULT_NAV_ICON = IconReportSearch;
/** Icon keys a workflow may pick for `ui.nav.icon` (the builder's picker reads
 *  this so its options stay in sync with what the sidebar can actually render). */
export const NAV_ICON_KEYS = Object.keys(NAV_ICONS);
function isActive(pathname: string, item: NavItem): boolean {
  return (
    pathname === item.href || (!!item.match && pathname.startsWith(item.match))
  );
}

/**
 * The sidebar's entries, in order: the harness's own tools, then the report
 * pages workflows declare.
 *
 * Exported as a hook because Settings → Preferences lists the same entries to
 * show and hide them, and the workflow-declared ones are only knowable at
 * runtime — so the two views cannot share a constant.
 */
export function useOperationsNav(): NavItem[] {
  // A workflow's page is only useful once it has a run — show these nav entries
  // conditionally (the run list is cached/shared across pages).
  const runs = useRuns({});
  const workflows = useWorkflowList();
  const hasRun = (name: string) =>
    !!runs.data?.some((r) => r.workflow_name === name);

  // Every nav entry is workflow-declared (`ui.nav`), shown once the workflow
  // has at least one run, linking to its generic report list page.
  const declaredNav: NavItem[] = (workflows.data ?? [])
    .filter((w) => w.ui?.nav && hasRun(w.name))
    .map((w) => ({
      href: `/reports/${w.name}`,
      label: w.ui!.nav!.label,
      icon: NAV_ICONS[w.ui!.nav!.icon ?? ""] ?? DEFAULT_NAV_ICON,
      match: `/reports/${w.name}`,
      group: "reports",
    }));

  return [
    {
      href: "/",
      label: "Dashboard",
      icon: IconLayoutDashboard,
      group: "harness",
    },
    {
      href: "/runs",
      label: "Runs",
      icon: IconRocket,
      match: "/runs",
      group: "harness",
    },
    // Hideable like the rest (Settings → Preferences); the header's Ask
    // button opens the same conversations in a side panel.
    {
      href: "/chat",
      label: "Chat",
      icon: IconMessageCircle,
      group: "harness",
    },
    // Linear triggers, webhooks and schedules: everything that starts a run
    // by itself.
    {
      href: "/triggers",
      label: "Triggers",
      icon: IconBolt,
      match: "/triggers",
      group: "harness",
    },
    // Last of the tools: a comparison is occasional work, not daily.
    {
      href: "/ab",
      label: "A/B Tests",
      icon: IconGitCompare,
      match: "/ab",
      group: "harness",
    },
    ...declaredNav,
  ];
}

/**
 * Left navigation: what you watch.
 *
 * Everything you author or configure lives in Settings, reached from the footer
 * — including which of these entries are shown, which used to be an edit mode
 * inside this component.
 */
export function AppSidebar({ ...props }: React.ComponentProps<typeof Sidebar>) {
  const { pathname } = useLocation();
  const { isHidden } = useHiddenNav();
  const items = useOperationsNav().filter((i) => !isHidden(i.href));

  return (
    <Sidebar collapsible="offcanvas" {...props}>
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton
              render={<Link to="/" />}
              className="w-fit data-[slot=sidebar-menu-button]:p-1.5!"
              title="Dashboard"
            >
              {/* A mark, not a wordmark: the harness is a factory for work,
                  and the name is in the browser tab already. */}
              <Factory className="size-5!" aria-hidden="true" />
              <span className="sr-only">ai-harness</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>
      <SidebarContent>
        {NAV_GROUPS.map(({ group, label }) => {
          const entries = items.filter((i) => i.group === group);
          if (entries.length === 0) return null;
          return (
            <SidebarGroup key={group}>
              {label && <SidebarGroupLabel>{label}</SidebarGroupLabel>}
              <SidebarGroupContent>
                <SidebarMenu>
                  {entries.map((item) => (
                    <SidebarMenuItem key={item.href}>
                      <SidebarMenuButton
                        render={<Link to={item.href} />}
                        isActive={isActive(pathname, item)}
                      >
                        <item.icon />
                        <span>{item.label}</span>
                      </SidebarMenuButton>
                    </SidebarMenuItem>
                  ))}
                </SidebarMenu>
              </SidebarGroupContent>
            </SidebarGroup>
          );
        })}
      </SidebarContent>
      <SidebarFooter>
        <AccountMenu />
      </SidebarFooter>
    </Sidebar>
  );
}
