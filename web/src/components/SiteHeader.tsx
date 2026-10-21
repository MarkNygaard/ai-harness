import { IconMessageCircle } from "@tabler/icons-react";
import { useLocation } from "react-router-dom";
import { useChatOpen } from "@/components/chat/ChatProvider";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { SidebarTrigger } from "@/components/ui/sidebar";

/**
 * Opens the chat panel. In the header so it is on every page — except the
 * Chat page itself, where the panel would be a second copy of what is open.
 */
function ChatButton() {
  const { setOpen } = useChatOpen();
  const { pathname } = useLocation();
  if (pathname === "/chat") return null;
  return (
    <Button
      variant="ghost"
      size="sm"
      onClick={() => setOpen(true)}
      aria-label="Open chat"
    >
      <IconMessageCircle />
      Ask
    </Button>
  );
}

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
      <ChatButton />
    </header>
  );
}
