import { Link } from "react-router-dom";
import { IconLogout, IconSettings } from "@tabler/icons-react";
import {
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/components/ui/sidebar";
import { useAuthStatus, useLogout } from "@/lib/auth";

/**
 * The way into Settings, and out of the harness.
 *
 * Not a dropdown: with no accounts there is only one entry, and with accounts
 * there are two. It becomes a menu when there is something to put in one.
 */
export function AccountMenu() {
  const status = useAuthStatus();
  const logout = useLogout();
  const user = status.data?.user;

  return (
    <SidebarMenu>
      {user && (
        <SidebarMenuItem>
          <div className="px-2 py-1">
            <div className="truncate text-xs font-medium">{user.name}</div>
            <div className="truncate text-[10px] text-muted-foreground">
              {user.email}
              {user.role === "admin" && " · admin"}
            </div>
          </div>
        </SidebarMenuItem>
      )}
      <SidebarMenuItem>
        <SidebarMenuButton render={<Link to="/settings/preferences" />}>
          <IconSettings />
          <span>Settings</span>
        </SidebarMenuButton>
      </SidebarMenuItem>
      {user && (
        <SidebarMenuItem>
          <SidebarMenuButton
            onClick={() => logout.mutate()}
            disabled={logout.isPending}
            className="text-muted-foreground"
          >
            <IconLogout />
            <span>{logout.isPending ? "Signing out…" : "Sign out"}</span>
          </SidebarMenuButton>
        </SidebarMenuItem>
      )}
    </SidebarMenu>
  );
}
