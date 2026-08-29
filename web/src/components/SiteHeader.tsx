import { Separator } from "@/components/ui/separator";
import { SidebarTrigger } from "@/components/ui/sidebar";

/** Top bar inside the sidebar inset: trigger + page title + optional actions. */
export function SiteHeader({
  title,
  actions,
}: {
  title: React.ReactNode;
  actions?: React.ReactNode;
}) {
  return (
    <header className="flex h-(--header-height) shrink-0 items-center gap-2 border-b px-4">
      <SidebarTrigger className="-ml-1" />
      {/* Base UI marks orientation with a bare `data-vertical`, not
          `data-orientation="vertical"` — so the old override never matched and
          the component's own `self-stretch` ran the rule the full height of the
          header, hard against the top edge. A short centred rule is what this
          was always meant to be. */}
      <Separator
        orientation="vertical"
        className="mr-1 data-vertical:h-4 data-vertical:self-center"
      />
      <div className="min-w-0 flex-1 truncate text-sm font-medium">{title}</div>
      {actions && <div className="flex items-center gap-2">{actions}</div>}
    </header>
  );
}
