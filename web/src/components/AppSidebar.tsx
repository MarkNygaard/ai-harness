import { Link, useLocation } from "react-router-dom";
import {
  IconBinaryTree2,
  IconClipboardCheck,
  IconFolderCog,
  IconGitCompare,
  IconHexagonalPrism,
  IconKey,
  IconLayoutDashboard,
  IconReportSearch,
  IconRocket,
  IconSearch,
  IconShieldCheck,
  IconTags,
  IconWorldSearch,
  IconZoomCode,
} from "@tabler/icons-react";
import { useRuns } from "@/lib/runs";
import { useWorkflowList } from "@/lib/authoring";
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

interface NavItem {
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
// Everything you author or configure — the building blocks you define and the
// system settings — kept out of Operations (which is run-observation only).
const MANAGE: NavItem[] = [
  {
    href: "/projects",
    label: "Projects",
    icon: IconFolderCog,
    match: "/projects",
  },
  {
    href: "/editor",
    label: "Workflows",
    icon: IconBinaryTree2,
    match: "/editor",
  },
  {
    href: "/credentials",
    label: "Credentials",
    icon: IconKey,
    match: "/credentials",
  },
  {
    href: "/categories",
    label: "Categories",
    icon: IconTags,
    match: "/categories",
  },
];
function isActive(pathname: string, item: NavItem): boolean {
  return (
    pathname === item.href || (!!item.match && pathname.startsWith(item.match))
  );
}

/** Left navigation, mirroring home-ops-agent's sidebar (base-nova). */
export function AppSidebar({ ...props }: React.ComponentProps<typeof Sidebar>) {
  const { pathname } = useLocation();
  // A workflow's page is only useful once it has a run — show these nav entries
  // conditionally (the run list is cached/shared across pages).
  const runs = useRuns({});
  const workflows = useWorkflowList();
  const hasRun = (name: string) =>
    !!runs.data?.some((r) => r.workflow_name === name);
  const hasGeoAudit = hasRun("geo-audit");
  const hasReview = hasRun("review-area");

  // Workflow-declared nav entries (`ui.nav`), shown once the workflow has run.
  // geo-audit / review-area keep their bespoke entries below until they migrate
  // to `ui`, so they're excluded here to avoid a duplicate.
  const declaredNav: NavItem[] = (workflows.data ?? [])
    .filter(
      (w) =>
        w.ui?.nav &&
        w.name !== "geo-audit" &&
        w.name !== "review-area" &&
        hasRun(w.name),
    )
    .map((w) => ({
      href: `/reports/${w.name}`,
      label: w.ui!.nav!.label,
      icon: NAV_ICONS[w.ui!.nav!.icon ?? ""] ?? DEFAULT_NAV_ICON,
      match: `/reports/${w.name}`,
    }));

  const operations: NavItem[] = [
    { href: "/", label: "Dashboard", icon: IconLayoutDashboard },
    { href: "/runs", label: "Runs", icon: IconRocket, match: "/runs" },
    { href: "/ab", label: "A/B Tests", icon: IconGitCompare, match: "/ab" },
    ...(hasGeoAudit
      ? [
          {
            href: "/geo",
            label: "GEO Audit",
            icon: IconWorldSearch,
            match: "/geo",
          } as NavItem,
        ]
      : []),
    ...(hasReview
      ? [
          {
            href: "/reviews",
            label: "Code Review",
            icon: IconZoomCode,
            match: "/reviews",
          } as NavItem,
        ]
      : []),
    ...declaredNav,
  ];

  const group = (label: string, items: NavItem[]) => (
    <SidebarGroup>
      <SidebarGroupLabel>{label}</SidebarGroupLabel>
      <SidebarGroupContent>
        <SidebarMenu>
          {items.map((item) => (
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
        {group("Operations", operations)}
        {group("Manage", MANAGE)}
      </SidebarContent>
      <SidebarFooter>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton className="cursor-default hover:bg-transparent">
              <span className="size-2 rounded-full bg-status-success" />
              <span className="text-xs text-muted-foreground">local</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarFooter>
    </Sidebar>
  );
}
