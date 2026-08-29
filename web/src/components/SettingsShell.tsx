import { Link } from "react-router-dom";
import { AppShell } from "@/components/AppShell";
import { SettingsSidebar } from "@/components/SettingsSidebar";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";

/**
 * The settings frame: the application shell with its nav replaced.
 *
 * A thin wrapper rather than a second layout, so every settings section gets
 * the same header, scroll behaviour and `viewActions` handling the rest of the
 * app already has.
 *
 * The header reads `Settings / <section>` rather than the section alone.
 * Settings is a mode you enter, not a page you visit — the sidebar changes
 * under you — and a lone "Categories" says nothing about where that is. Pages
 * still pass their own name; the trail is added here so no section has to
 * remember to.
 */
export function SettingsShell({
  title,
  ...props
}: Omit<React.ComponentProps<typeof AppShell>, "sidebar">) {
  return (
    <AppShell
      {...props}
      title={
        <Breadcrumb>
          <BreadcrumbList className="sm:gap-1.5">
            <BreadcrumbItem>
              <BreadcrumbLink render={<Link to="/settings" />}>
                Settings
              </BreadcrumbLink>
            </BreadcrumbItem>
            <BreadcrumbSeparator />
            <BreadcrumbItem className="min-w-0">
              <BreadcrumbPage className="truncate font-medium text-foreground">
                {title}
              </BreadcrumbPage>
            </BreadcrumbItem>
          </BreadcrumbList>
        </Breadcrumb>
      }
      sidebar={<SettingsSidebar variant="inset" />}
    />
  );
}
