import { Link, useLocation } from "react-router-dom";
import {
  IconClipboardCheck,
  IconGitCompare,
  IconHexagonalPrism,
  IconLayoutDashboard,
  IconReportSearch,
  IconRocket,
  IconSearch,
  IconShieldCheck,
  IconWorldSearch,
  IconZoomCode,
} from "@tabler/icons-react";
import { useHiddenNav } from "@/lib/nav-prefs";
import { useRuns } from "@/lib/runs";
import { useWorkflowList } from "@/lib/authoring";
import { AccountMenu } from "@/components/AccountMenu";
import { ClaudeCodeVersion } from "@/components/ClaudeCodeVersion";
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

export interface NavItem {
  href: string;
  label: string;
  icon: typeof IconRocket;
  /** Active when the path equals href, or (for prefixes) starts with `match`. */
  match?: string;
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
 * The Operations entries, in sidebar order.
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
    }));

  return [
    { href: "/", label: "Dashboard", icon: IconLayoutDashboard },
    { href: "/runs", label: "Runs", icon: IconRocket, match: "/runs" },
    { href: "/ab", label: "A/B Tests", icon: IconGitCompare, match: "/ab" },
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
  const operations = useOperationsNav().filter((i) => !isHidden(i.href));

  return (
    <Sidebar collapsible="offcanvas" {...props}>
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton
              render={<Link to="/" />}
              className="data-[slot=sidebar-menu-button]:p-1.5!"
            >
              <IconHexagonalPrism className="size-5!" />
              <span className="text-base font-semibold">ai-harness</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>
      <SidebarContent>
        {operations.length > 0 && (
          <SidebarGroup>
            <SidebarGroupLabel>Operations</SidebarGroupLabel>
            <SidebarGroupContent>
              <SidebarMenu>
                {operations.map((item) => (
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
        )}
      </SidebarContent>
      <SidebarFooter>
        <AccountMenu />
        <ClaudeCodeVersion />
      </SidebarFooter>
    </Sidebar>
  );
}
