import { useEffect } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { useAuthStatus } from "@/lib/auth";

/**
 * Pages reachable without being signed in.
 *
 * Not "pages that are how you sign in" — that framing is what made this list
 * wrong. It is *every* page someone who has no account can legitimately be
 * looking at, and an invitation and a password reset are exactly that: links
 * sent to people who cannot sign in, by definition. Leaving them out sent the
 * holder of a fresh invite straight to a login form for the account the link
 * was going to create.
 *
 * The invite path carries its token as a segment, so it matches by prefix.
 */
export function isPublicPath(pathname: string): boolean {
  return (
    pathname === "/login" ||
    pathname === "/setup" ||
    pathname === "/forgot" ||
    pathname.startsWith("/invite/")
  );
}

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
  const onPublicPage = isPublicPath(pathname);

  useEffect(() => {
    if (!needsSignIn || onPublicPage) return;
    // `replace`, so Back doesn't bounce between the app and the login page.
    navigate("/login", { replace: true });
  }, [needsSignIn, onPublicPage, navigate]);

  return null;
}
