import { useState } from "react";
import { Link, useLocation } from "react-router-dom";
import {
  IconArrowLeft,
  IconBinaryTree2,
  IconRobot,
  IconFolderCog,
  IconKey,
  IconMail,
  IconPlugConnected,
  IconSettings2,
  IconShieldLock,
  IconTags,
  IconUsers,
  IconWorld,
} from "@tabler/icons-react";
import { useAuthStatus } from "@/lib/auth";
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
  /**
   * Hide from members. Presentation only — every one of these routes rejects a
   * non-administrator server-side, which is what actually protects them.
   */
  adminOnly?: boolean;
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
        href: "/settings/general",
        label: "General",
        icon: IconWorld,
        keywords: "domain public url address hostname",
        adminOnly: true,
      },
      {
        href: "/settings/members",
        label: "Members",
        icon: IconUsers,
        keywords: "users people accounts admin role invite team",
        adminOnly: true,
      },
      {
        href: "/settings/sso",
        label: "Sign-in",
        icon: IconShieldLock,
        keywords: "sso oidc entra google okta keycloak login provider",
        adminOnly: true,
      },
      {
        href: "/settings/email",
        label: "Email",
        icon: IconMail,
        keywords: "smtp mail invite password reset relay",
        adminOnly: true,
      },
      {
        href: "/settings/agents",
        label: "Agents",
        icon: IconRobot,
        keywords: "claude code codex omp pi cursor cli version update provider",
        adminOnly: true,
      },
      {
        href: "/settings/subscriptions",
        label: "Subscriptions",
        icon: IconKey,
        keywords:
          "claude chatgpt openai kimi cursor credentials token oauth api key billing usage",
        adminOnly: true,
      },
      {
        href: "/settings/integrations",
        label: "Integrations",
        icon: IconPlugConnected,
        keywords: "github linear repo issue tracker oauth",
        adminOnly: true,
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
  const status = useAuthStatus();
  const [query, setQuery] = useState("");

  // Before an install has accounts there are no roles, so nothing is hidden —
  // whoever got in is the operator by definition.
  const isAdmin =
    status.data?.mode !== "accounts" || status.data?.user?.role === "admin";

  const groups = GROUPS.map((g) => ({
    ...g,
    items: g.items.filter(
      (i) => (isAdmin || !i.adminOnly) && matches(i, query),
    ),
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
