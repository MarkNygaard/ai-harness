import { createContext, useContext, useMemo, useState } from "react";

/**
 * Whether the chat panel is open. Shared so the button in every page's header
 * and the one panel mounted beside the routes agree.
 */
const ChatOpenContext = createContext<{
  open: boolean;
  setOpen: (open: boolean) => void;
} | null>(null);

export function ChatProvider({ children }: { children: React.ReactNode }) {
  const [open, setOpen] = useState(false);
  const value = useMemo(() => ({ open, setOpen }), [open]);
  return (
    <ChatOpenContext.Provider value={value}>
      {children}
    </ChatOpenContext.Provider>
  );
}

/**
 * The panel's open state. Outside a provider (a page rendered on its own in a
 * test) it reads as closed and opening does nothing.
 */
export function useChatOpen() {
  return (
    useContext(ChatOpenContext) ?? { open: false, setOpen: () => undefined }
  );
}
