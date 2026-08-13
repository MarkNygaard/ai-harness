import { AppSidebar } from "@/components/AppSidebar";
import { SiteHeader } from "@/components/SiteHeader";
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar";

/**
 * Application shell — shadcn `Sidebar` (inset variant) + a `SiteHeader` title
 * bar, mirroring home-ops-agent. Pages render their content as children.
 */
export function AppShell({
  title,
  actions,
  viewActions,
  children,
}: {
  title: React.ReactNode;
  /** Actions rendered in the header bar, beside the page title. */
  actions?: React.ReactNode;
  /**
   * Page-level actions rendered in the **view**, pinned to its top-right edge
   * rather than to the centred content column — so they sit flush right on wide
   * screens. This is the house pattern for a page's primary actions (Projects,
   * Workflows); prefer it over `actions` for anything that belongs to the page
   * rather than to the app frame.
   *
   * Because they are positioned out of flow from `sm` up, a page using this must
   * reserve room on its heading block (`sm:pr-64`) so long titles or
   * descriptions can't run underneath. Below `sm` they fall back into normal
   * flow, where there is no room to sit beside a heading anyway.
   */
  viewActions?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <SidebarProvider
      className="h-svh overflow-hidden"
      style={
        {
          "--sidebar-width": "calc(var(--spacing) * 60)",
          "--header-height": "calc(var(--spacing) * 12)",
        } as React.CSSProperties
      }
    >
      <AppSidebar variant="inset" />
      <SidebarInset className="flex min-h-0 flex-col overflow-hidden">
        <SiteHeader title={title} actions={actions} />
        <div className="min-h-0 flex-1 overflow-auto">
          {viewActions ? (
            <div className="relative">
              <div className="flex items-center justify-end gap-2 p-6 pb-0 sm:absolute sm:right-6 sm:top-6 sm:z-10 sm:p-0">
                {viewActions}
              </div>
              {children}
            </div>
          ) : (
            children
          )}
        </div>
      </SidebarInset>
    </SidebarProvider>
  );
}
