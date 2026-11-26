import { useEffect, useRef } from "react";
import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { useInbox, useMarkRead } from "@/lib/notifications";

/**
 * A toast for each notification that arrives while the app is open.
 *
 * What is already in the inbox when the page loads is not announced again:
 * that is what the inbox and its count are for. Renders nothing.
 */
export function NotificationToasts() {
  const inbox = useInbox();
  const markRead = useMarkRead();
  const navigate = useNavigate();
  // The newest id seen so far; `null` until the first answer, which only sets it.
  const newest = useRef<number | null>(null);

  useEffect(() => {
    const rows = inbox.data?.notifications;
    if (!rows) return;
    const top = rows.reduce((m, n) => Math.max(m, n.id), 0);
    if (newest.current === null) {
      newest.current = top;
      return;
    }
    const fresh = rows.filter((n) => n.id > newest.current! && !n.read_at);
    newest.current = Math.max(newest.current, top);
    for (const n of fresh.slice(0, 3).reverse()) {
      toast(n.title, {
        id: `notification-${n.id}`,
        description: n.body || undefined,
        action: n.link
          ? {
              label: "Open",
              onClick: () => {
                markRead.mutate([n.id]);
                navigate(n.link!);
              },
            }
          : undefined,
      });
    }
  }, [inbox.data, markRead, navigate]);

  return null;
}
