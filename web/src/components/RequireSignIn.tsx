import { useEffect } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { useAuthStatus } from "@/lib/auth";

/** Pages you can reach without being signed in — they are how you sign in. */
const PUBLIC = ["/login", "/setup"];

/**
 * Send an unauthenticated visitor to the right door.
 *
 * Only acts in `accounts` mode: `open` and `token` installs behave exactly as
 * they always have, and this renders nothing at all. It sits beside the router
 * rather than wrapping every route, so nothing has to be restructured to add it
 * — and so the redirect happens once, from one place.
 */
export function RequireSignIn() {
  const status = useAuthStatus();
  const navigate = useNavigate();
  const { pathname } = useLocation();

  const needsSignIn = status.data?.mode === "accounts" && !status.data.user;
  const onPublicPage = PUBLIC.includes(pathname);

  useEffect(() => {
    if (!needsSignIn || onPublicPage) return;
    // `replace`, so Back doesn't bounce between the app and the login page.
    navigate("/login", { replace: true });
  }, [needsSignIn, onPublicPage, navigate]);

  return null;
}
