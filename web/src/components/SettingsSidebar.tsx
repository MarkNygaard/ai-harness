import { useState } from "react";
import { Link, useLocation } from "react-router-dom";
import {
  IconArrowLeft,
  IconBinaryTree2,
  IconFolderCog,
  IconKey,
  IconPlugConnected,
  IconSettings2,
  IconTags,
} from "@tabler/icons-react";
import {
  Sidebar,
  SidebarContent,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/components/ui/sidebar";

interface SettingsNavItem {
  href: string;
  label: string;
  icon: typeof IconKey;
  /** Words that should match this entry beyond its label. */
  keywords?: string;
}

/**
 * Settings navigation, in three groups.
 *
 * **Account** is about you, **Workspace** is about this installation, and
 * **Building blocks** is what runs are made of. Sections arrive here as the
 * access work lands — Profile and Editor connection under Account, and General,
 * Members, Sign-in and Email under Workspace.
 */
const GROUPS: { label: string; items: SettingsNavItem[] }[] = [
  {
    label: "Account",
    items: [
      {
        href: "/settings/preferences",
        label: "Preferences",
        icon: IconSettings2,
        keywords: "theme appearance dark light sidebar menu",
      },
      {
        href: "/settings/mcp",
        label: "Editor connection",
        icon: IconPlugConnected,
        keywords: "mcp claude code cursor vscode token key editor snippet",
      },
    ],
  },
  {
    label: "Workspace",
    items: [
      {
        href: "/settings/credentials",
        label: "Credentials",
        icon: IconKey,
        keywords: "claude codex cursor github linear token oauth api key",
      },
    ],
  },
  {
    label: "Building blocks",
    items: [
      {
        href: "/settings/projects",
        label: "Projects",
        icon: IconFolderCog,
        keywords: "repo repository git toolchain branch",
      },
      {
        href: "/settings/workflows",
        label: "Workflows",
        icon: IconBinaryTree2,
        keywords: "dag yaml editor nodes commands",
      },
      {
        href: "/settings/categories",
        label: "Categories",
        icon: IconTags,
        keywords: "steps grouping colour color",
      },
    ],
  },
];

function matches(item: SettingsNavItem, query: string): boolean {
  if (!query) return true;
  const haystack = `${item.label} ${item.keywords ?? ""}`.toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((term) => haystack.includes(term));
}

/**
 * Settings navigation. Entering settings **replaces** the app sidebar rather
 * than nesting inside it — configuring the harness is a different mode from
 * watching it work, and the back link is the way out.
 */
export function SettingsSidebar({
  ...props
}: React.ComponentProps<typeof Sidebar>) {
  const { pathname } = useLocation();
  const [query, setQuery] = useState("");

  const groups = GROUPS.map((g) => ({
    ...g,
    items: g.items.filter((i) => matches(i, query)),
  })).filter((g) => g.items.length > 0);

  return (
    <Sidebar collapsible="offcanvas" {...props}>
      <SidebarHeader className="gap-2">
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton
              render={<Link to="/" />}
              className="text-muted-foreground"
            >
              <IconArrowLeft />
              <span>Back to app</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search settings…"
          aria-label="Search settings"
          className="h-8 w-full rounded-md border border-input bg-transparent px-2 text-[13px] outline-none focus:ring-2 focus:ring-ring"
        />
      </SidebarHeader>
      <SidebarContent>
        {groups.map((g) => (
          <SidebarGroup key={g.label}>
            <SidebarGroupLabel>{g.label}</SidebarGroupLabel>
            <SidebarGroupContent>
              <SidebarMenu>
                {g.items.map((item) => (
                  <SidebarMenuItem key={item.href}>
                    <SidebarMenuButton
                      render={<Link to={item.href} />}
                      isActive={pathname.startsWith(item.href)}
                    >
                      <item.icon />
                      <span>{item.label}</span>
                    </SidebarMenuButton>
                  </SidebarMenuItem>
                ))}
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>
        ))}
        {groups.length === 0 && (
          <p className="px-4 py-2 text-xs text-muted-foreground">
            Nothing matches “{query}”.
          </p>
        )}
      </SidebarContent>
    </Sidebar>
  );
}
