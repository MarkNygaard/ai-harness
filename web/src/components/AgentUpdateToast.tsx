import { useEffect, useRef } from "react";
import { Link, useLocation } from "react-router-dom";
import { ArrowUpCircle } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  describeAgentUpdates,
  markAgentVersionsSeen,
  pendingAgentUpdates,
  readSeenAgentVersions,
} from "@/lib/agents";
import { isAdminOf, useAuthStatus } from "@/lib/auth";
import { useProviderHealth } from "@/lib/system";

const TOAST_ID = "agent-updates";

/** The page the notice sends you to, and the one page it stays quiet on. */
const AGENTS_PATH = "/settings/agents";

/**
 * Tells an administrator, once per page load, that an agent CLI has a newer
 * release than the one this container is running.
 *
 * An agent going stale is the one thing about an installation that changes
 * without anyone deciding it should, so it is the one thing worth interrupting
 * someone about — but only someone who can act on it, and only until they say
 * they have seen it. Hence: administrators only, dismissable, and dismissal
 * remembered per version rather than forever.
 *
 * Renders nothing. It fires a toast into the host mounted alongside it.
 */
export function AgentUpdateToast() {
  const status = useAuthStatus();
  const { pathname } = useLocation();
  // Only an administrator can install the update, and `/api/system/providers`
  // is admin-only server-side — asking as a member would spend a rejected
  // request on every page load to learn nothing.
  const admin = status.isSuccess && isAdminOf(status.data);
  const health = useProviderHealth(admin);
  // Per component instance, which is per page load: a ref rather than a module
  // flag so the guard cannot leak between tests, and so React's development
  // double-invoke of this effect does not raise two toasts.
  const raised = useRef(false);

  useEffect(() => {
    if (raised.current || !admin || !health.data) return;
    // The Agents page already shows the version and the button beside it.
    if (pathname === AGENTS_PATH) return;

    const updates = pendingAgentUpdates(health.data, readSeenAgentVersions());
    if (updates.length === 0) return;
    raised.current = true;

    const { title, detail } = describeAgentUpdates(updates);
    // Both ways out of the toast count as having been told: whoever followed
    // the link has seen it, and if they do not install anything the server
    // keeps offering the update on the page they landed on.
    const remember = () => markAgentVersionsSeen(updates);

    toast(title, {
      id: TOAST_ID,
      description: detail,
      // Nothing here is urgent, and a notice that times out unread is a notice
      // that did not happen. It leaves when someone decides it does.
      duration: Infinity,
      closeButton: true,
      onDismiss: remember,
      action: (
        <Button
          size="sm"
          variant="outline"
          className="gap-1"
          render={<Link to={AGENTS_PATH} />}
          onClick={() => {
            remember();
            toast.dismiss(TOAST_ID);
          }}
        >
          <ArrowUpCircle className="size-3" /> Update
        </Button>
      ),
    });
  }, [admin, health.data, pathname]);

  return null;
}
