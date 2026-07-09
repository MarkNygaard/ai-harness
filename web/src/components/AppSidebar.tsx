import { useEffect, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import {
  IconBinaryTree2,
  IconClipboardCheck,
  IconEye,
  IconEyeOff,
  IconFolderCog,
  IconGitCompare,
  IconHexagonalPrism,
  IconKey,
  IconLayoutDashboard,
  IconPencil,
  IconReportSearch,
  IconRocket,
  IconSearch,
  IconShieldCheck,
  IconTags,
  IconWorldSearch,
  IconZoomCode,
} from "@tabler/icons-react";
import { cn } from "@/lib/utils";
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
  SidebarMenuAction,
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

/** Personal, per-browser set of nav hrefs the user has hidden from the menu. */
const HIDDEN_KEY = "harness.nav.hidden";
function loadHidden(): Set<string> {
  try {
    const raw = localStorage.getItem(HIDDEN_KEY);
    return new Set(raw ? (JSON.parse(raw) as string[]) : []);
  } catch {
    return new Set();
  }
}

/** Left navigation, mirroring home-ops-agent's sidebar (base-nova). */
export function AppSidebar({ ...props }: React.ComponentProps<typeof Sidebar>) {
  const { pathname } = useLocation();
  // Edit mode reveals a per-item eye toggle; the hidden set persists per browser.
  const [editing, setEditing] = useState(false);
  const [hidden, setHidden] = useState<Set<string>>(loadHidden);
  useEffect(() => {
    try {
      localStorage.setItem(HIDDEN_KEY, JSON.stringify([...hidden]));
    } catch {
      /* private mode / storage disabled — hiding just won't persist */
    }
  }, [hidden]);
  const toggleHidden = (href: string) =>
    setHidden((prev) => {
      const next = new Set(prev);
      if (next.has(href)) next.delete(href);
      else next.add(href);
      return next;
    });
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

  const operations: NavItem[] = [
    { href: "/", label: "Dashboard", icon: IconLayoutDashboard },
    { href: "/runs", label: "Runs", icon: IconRocket, match: "/runs" },
    { href: "/ab", label: "A/B Tests", icon: IconGitCompare, match: "/ab" },
    ...declaredNav,
  ];

  const group = (label: string, items: NavItem[]) => {
    // Out of edit mode, hidden items disappear; in edit mode every item shows
    // (dimmed if hidden) with an eye toggle. An all-hidden group vanishes too.
    const visible = editing ? items : items.filter((i) => !hidden.has(i.href));
    if (visible.length === 0) return null;
    return (
      <SidebarGroup>
        <SidebarGroupLabel>{label}</SidebarGroupLabel>
        <SidebarGroupContent>
          <SidebarMenu>
            {visible.map((item) => {
              const isHidden = hidden.has(item.href);
              return (
                <SidebarMenuItem key={item.href}>
                  <SidebarMenuButton
                    render={<Link to={item.href} />}
                    isActive={isActive(pathname, item)}
                    className={cn(editing && isHidden && "opacity-40")}
                  >
                    <item.icon />
                    <span>{item.label}</span>
                  </SidebarMenuButton>
                  {editing && (
                    <SidebarMenuAction
                      onClick={() => toggleHidden(item.href)}
                      aria-label={isHidden ? "Show in menu" : "Hide from menu"}
                      title={isHidden ? "Show in menu" : "Hide from menu"}
                    >
                      {isHidden ? <IconEyeOff /> : <IconEye />}
                    </SidebarMenuAction>
                  )}
                </SidebarMenuItem>
              );
            })}
          </SidebarMenu>
        </SidebarGroupContent>
      </SidebarGroup>
    );
  };

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
            <SidebarMenuButton
              onClick={() => setEditing((v) => !v)}
              isActive={editing}
              className="text-xs text-muted-foreground"
              title={
                editing
                  ? "Done — hidden items are now tucked away"
                  : "Show / hide menu items"
              }
            >
              <IconPencil />
              <span>{editing ? "Done editing menu" : "Edit menu"}</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
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
