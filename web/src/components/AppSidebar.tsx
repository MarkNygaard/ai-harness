import { Link, useLocation } from "react-router-dom";
import {
  IconBinaryTree2,
  IconFolderCog,
  IconHexagonalPrism,
  IconKey,
  IconRocket,
} from "@tabler/icons-react";
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

const OPERATIONS: NavItem[] = [
  { href: "/", label: "Runs", icon: IconRocket, match: "/runs" },
  { href: "/projects", label: "Projects", icon: IconFolderCog, match: "/projects" },
  { href: "/editor", label: "Workflows", icon: IconBinaryTree2, match: "/editor" },
];

const SYSTEM: NavItem[] = [
  { href: "/credentials", label: "Credentials", icon: IconKey, match: "/credentials" },
];

function isActive(pathname: string, item: NavItem): boolean {
  if (item.href === "/") return pathname === "/" || pathname.startsWith("/runs");
  return pathname === item.href || (item.match ? pathname.startsWith(item.match) : false);
}

/** Left navigation, mirroring home-ops-agent's sidebar (base-nova). */
export function AppSidebar({ ...props }: React.ComponentProps<typeof Sidebar>) {
  const { pathname } = useLocation();

  const group = (label: string, items: NavItem[]) => (
    <SidebarGroup>
      <SidebarGroupLabel>{label}</SidebarGroupLabel>
      <SidebarGroupContent>
        <SidebarMenu>
          {items.map((item) => (
            <SidebarMenuItem key={item.href}>
              <SidebarMenuButton render={<Link to={item.href} />} isActive={isActive(pathname, item)}>
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
        {group("Operations", OPERATIONS)}
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
