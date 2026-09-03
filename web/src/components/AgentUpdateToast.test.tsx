import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { toast } from "sonner";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AgentUpdateToast } from "./AgentUpdateToast";
import { Toaster } from "./ui/sonner";
import { AGENT_UPDATES_SEEN_KEY, readSeenAgentVersions } from "@/lib/agents";
import type { AuthStatus } from "@/lib/auth";
import type { ProviderHealth } from "@/lib/system";

const ADMIN: AuthStatus = {
  mode: "accounts",
  claimed: true,
  min_password_len: 12,
  user: {
    id: "u/1",
    email: "ada@x.dev",
    name: "Ada",
    role: "admin",
    created_at: "2025-01-01T00:00:00Z",
    last_login_at: null,
    disabled_at: null,
  },
};

const MEMBER: AuthStatus = {
  ...ADMIN,
  user: { ...ADMIN.user!, id: "u/2", role: "member" },
};

const STALE_CLAUDE: ProviderHealth = {
  provider: "claude",
  binary: "claude",
  on_path: true,
  version: "2.0.9",
  latest: "2.1.4",
  update_available: true,
  error: null,
};

/**
 * Answer the two queries the notice depends on, and record what was asked —
 * "was `/api/system/providers` requested at all" is itself one of the
 * behaviours under test.
 */
function serve(status: AuthStatus, providers: ProviderHealth[]) {
  const asked: string[] = [];
  global.fetch = vi.fn(async (input: RequestInfo | URL) => {
    const path = String(input);
    asked.push(path);
    if (path.startsWith("/api/auth/status")) {
      return new Response(JSON.stringify(status));
    }
    if (path.startsWith("/api/system/providers")) {
      return new Response(JSON.stringify(providers));
    }
    return new Response("{}", { status: 404 });
  }) as unknown as typeof fetch;
  return asked;
}

/**
 * That the notice never appears, rather than that it has not appeared yet — the
 * queries it waits on settle asynchronously, so a plain `queryByText` would
 * pass on a notice that is merely one tick away.
 */
async function expectNoNotice() {
  await expect(
    screen.findByText("Claude Code 2.1.4 is available"),
  ).rejects.toThrow();
}

function mount(at = "/") {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[at]}>
        <AgentUpdateToast />
        <Toaster />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("<AgentUpdateToast>", () => {
  const originalFetch = global.fetch;

  beforeEach(() => {
    localStorage.clear();
    // jsdom implements neither, and sonner's swipe-to-dismiss handler calls
    // them on the first pointer event over a toast.
    Element.prototype.setPointerCapture = () => {};
    Element.prototype.releasePointerCapture = () => {};
  });

  afterEach(() => {
    global.fetch = originalFetch;
    // Sonner's store outlives the component: a subscribing `<Toaster />` is
    // replayed every toast still active, so one left standing here reappears
    // in the next test's document.
    toast.dismiss();
    vi.restoreAllMocks();
  });

  it("tells an administrator, and offers the page that has the button", async () => {
    serve(ADMIN, [STALE_CLAUDE]);
    mount();

    expect(
      await screen.findByText("Claude Code 2.1.4 is available"),
    ).toBeInTheDocument();
    expect(screen.getByText("This container runs 2.0.9.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /update/i })).toHaveAttribute(
      "href",
      "/settings/agents",
    );
  });

  it("says nothing to a member, and does not ask an admin-only route", async () => {
    const asked = serve(MEMBER, [STALE_CLAUDE]);
    mount();

    await waitFor(() =>
      expect(asked.some((p) => p.startsWith("/api/auth/status"))).toBe(true),
    );
    expect(asked.some((p) => p.startsWith("/api/system/providers"))).toBe(
      false,
    );
    await expectNoNotice();
  });

  it("stays quiet on the Agents page, which already shows the update", async () => {
    const asked = serve(ADMIN, [STALE_CLAUDE]);
    mount("/settings/agents");

    await waitFor(() =>
      expect(asked.some((p) => p.startsWith("/api/system/providers"))).toBe(
        true,
      ),
    );
    await expectNoNotice();
    // Nothing was shown, so nothing may have been recorded as shown.
    expect(readSeenAgentVersions()).toEqual({});
  });

  it("remembers the version once dismissed, so the next load is quiet", async () => {
    serve(ADMIN, [STALE_CLAUDE]);
    const u = userEvent.setup();
    mount();

    await screen.findByText("Claude Code 2.1.4 is available");
    await u.click(screen.getByRole("button", { name: /close/i }));

    await waitFor(() =>
      expect(readSeenAgentVersions()).toEqual({ claude: "2.1.4" }),
    );
  });

  it("does not raise a notice for a version already dismissed", async () => {
    localStorage.setItem(
      AGENT_UPDATES_SEEN_KEY,
      JSON.stringify({ claude: "2.1.4" }),
    );
    const asked = serve(ADMIN, [STALE_CLAUDE]);
    mount();

    await waitFor(() =>
      expect(asked.some((p) => p.startsWith("/api/system/providers"))).toBe(
        true,
      ),
    );
    await expectNoNotice();
  });
});
