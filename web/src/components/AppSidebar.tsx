import { Link, useLocation } from "react-router-dom";
import {
  IconBinaryTree2,
  IconFolderCog,
  IconGitCompare,
  IconHexagonalPrism,
  IconKey,
  IconLayoutDashboard,
  IconRocket,
  IconTags,
  IconWorldSearch,
} from "@tabler/icons-react";
import { useRuns } from "@/lib/runs";
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
const SYSTEM: NavItem[] = [
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
  // The GEO Audit page is only useful once a geo-audit run exists — show its
  // nav entry conditionally (the run list is cached/shared across pages).
  const runs = useRuns({});
  const hasGeoAudit = !!runs.data?.some((r) => r.workflow_name === "geo-audit");

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
        {group("System", SYSTEM)}
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
