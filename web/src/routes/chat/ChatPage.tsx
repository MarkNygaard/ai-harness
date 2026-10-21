import { AppShell } from "@/components/AppShell";
import { ChatView } from "@/components/chat/ChatPanel";

/**
 * The chat as a page of its own, for a longer conversation than the side
 * panel is comfortable for. Same conversations, same remembered place.
 */
export function ChatPage() {
  return (
    <AppShell title="Chat">
      <div className="mx-auto flex h-full max-w-3xl flex-col">
        <ChatView variant="page" />
      </div>
    </AppShell>
  );
}
