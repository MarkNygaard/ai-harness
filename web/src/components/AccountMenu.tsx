import { Link } from "react-router-dom";
import { IconChevronUp, IconSettings } from "@tabler/icons-react";
import {
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/components/ui/sidebar";
import {
  Menu,
  MenuContent,
  MenuItem,
  MenuSeparator,
  MenuTrigger,
} from "@/components/ui/menu";
import { useAuthStatus, useLogout } from "@/lib/auth";

/**
 * Who you are, and everything that follows from that.
 *
 * Pressing your name opens the menu, rather than the entries sitting in the
 * footer permanently: they are things you do occasionally, and the sidebar is
 * for the things you do constantly.
 *
 * No icons on the items. A three-entry menu of short phrases is read, not
 * scanned, and an icon beside each one would be decoration competing with the
 * navigation above it for the same glance.
 */
export function AccountMenu() {
  const status = useAuthStatus();
  const logout = useLogout();
  const user = status.data?.user;

  // Before the harness is claimed there is no name to press and nothing to
  // sign out of. One entry stays one entry rather than hiding behind a menu.
  if (!user) {
    return (
      <SidebarMenu>
        <SidebarMenuItem>
          <SidebarMenuButton render={<Link to="/settings/preferences" />}>
            <IconSettings />
            <span>Settings</span>
          </SidebarMenuButton>
        </SidebarMenuItem>
      </SidebarMenu>
    );
  }

  const isAdmin = user.role === "admin";

  return (
    <SidebarMenu>
      <SidebarMenuItem>
        <Menu>
          <MenuTrigger
            render={<SidebarMenuButton size="lg" tooltip={user.email} />}
          >
            <div className="flex min-w-0 flex-1 flex-col text-left">
              <span className="truncate text-xs font-medium">{user.name}</span>
              <span className="truncate text-[10px] text-muted-foreground">
                {user.email}
                {isAdmin && " · admin"}
              </span>
            </div>
            <IconChevronUp className="shrink-0 text-muted-foreground" />
          </MenuTrigger>

          <MenuContent>
            <MenuItem render={<Link to="/settings/preferences" />}>
              Settings
            </MenuItem>
            {/* Members is administrator-only, so offering it to everyone else
                would be a route to a page they cannot open. */}
            {isAdmin && (
              <MenuItem render={<Link to="/settings/members" />}>
                Invite and manage members
              </MenuItem>
            )}
            <MenuSeparator />
            <MenuItem
              onClick={() => logout.mutate()}
              disabled={logout.isPending}
              // Signing out is a request in flight, not a navigation: keep the
              // menu open long enough to say so.
              closeOnClick={false}
            >
              {logout.isPending ? "Signing out…" : "Sign out"}
            </MenuItem>
          </MenuContent>
        </Menu>
      </SidebarMenuItem>
    </SidebarMenu>
  );
}
