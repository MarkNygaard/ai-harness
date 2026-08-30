import * as React from "react";
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemberProfileDialog } from "./MemberProfileDialog";
import type { AuthUser } from "@/lib/auth";

const userFixture: AuthUser = {
  id: "u/1",
  email: "ada@x.dev",
  name: "Ada",
  role: "member",
  created_at: "2025-01-01T00:00:00Z",
  last_login_at: null,
  disabled_at: null,
};

function renderWithClient(ui: React.ReactElement) {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  return render(
    <QueryClientProvider client={client}>{ui}</QueryClientProvider>,
  );
}

describe("<MemberProfileDialog>", () => {
  const originalFetch = global.fetch;

  beforeEach(() => {
    localStorage.clear();
  });

  afterEach(() => {
    global.fetch = originalFetch;
    vi.restoreAllMocks();
  });

  it("opens pre-filled with the member's name and email", async () => {
    global.fetch = vi.fn().mockResolvedValue(new Response("{}"));
    const u = userEvent.setup();
    renderWithClient(<MemberProfileDialog user={userFixture} busy={false} />);

    await u.click(screen.getByRole("button", { name: /edit/i }));

    expect(await screen.findByLabelText(/name/i)).toHaveValue("Ada");
    expect(screen.getByLabelText(/email/i)).toHaveValue("ada@x.dev");
  });

  it("saves, PUTs the new email, and closes", async () => {
    const updated: AuthUser = { ...userFixture, email: "new@x.dev" };
    const mock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify(updated)));
    global.fetch = mock;
    const u = userEvent.setup();
    renderWithClient(<MemberProfileDialog user={userFixture} busy={false} />);

    await u.click(screen.getByRole("button", { name: /edit/i }));
    const emailInput = await screen.findByLabelText(/email/i);
    await u.clear(emailInput);
    await u.type(emailInput, "new@x.dev");
    await u.click(screen.getByRole("button", { name: /save changes/i }));

    await waitFor(() => expect(mock).toHaveBeenCalledOnce());
    const [url, init] = mock.mock.calls[0];
    expect(url).toBe("/api/users/u%2F1");
    expect((init as RequestInit).method).toBe("PUT");
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({
      name: "Ada",
      email: "new@x.dev",
    });

    await waitFor(() =>
      expect(screen.queryByLabelText(/email/i)).not.toBeInTheDocument(),
    );
  });

  it("renders the server's 409 sentence and keeps the dialog open", async () => {
    const mock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          error: "new@x.dev already belongs to another account",
        }),
        { status: 409 },
      ),
    );
    global.fetch = mock;
    const u = userEvent.setup();
    renderWithClient(<MemberProfileDialog user={userFixture} busy={false} />);

    await u.click(screen.getByRole("button", { name: /edit/i }));
    const emailInput = await screen.findByLabelText(/email/i);
    await u.clear(emailInput);
    await u.type(emailInput, "new@x.dev");
    await u.click(screen.getByRole("button", { name: /save changes/i }));

    expect(
      await screen.findByText(/already belongs to another account/i),
    ).toBeInTheDocument();
    expect(screen.queryByText(/HTTP 409/i)).not.toBeInTheDocument();
    expect(screen.getByLabelText(/email/i)).toBeInTheDocument();
  });

  it("does not call fetch when cancelled", async () => {
    const mock = vi.fn().mockResolvedValue(new Response("{}"));
    global.fetch = mock;
    const u = userEvent.setup();
    renderWithClient(<MemberProfileDialog user={userFixture} busy={false} />);

    await u.click(screen.getByRole("button", { name: /edit/i }));
    const emailInput = await screen.findByLabelText(/email/i);
    await u.type(emailInput, "new@x.dev");
    await u.click(screen.getByRole("button", { name: /cancel/i }));

    expect(mock).not.toHaveBeenCalled();
    await waitFor(() =>
      expect(screen.queryByLabelText(/email/i)).not.toBeInTheDocument(),
    );
  });

  it("clears a stale error and resets fields on reopen", async () => {
    const mock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          error: "new@x.dev already belongs to another account",
        }),
        { status: 409 },
      ),
    );
    global.fetch = mock;
    const u = userEvent.setup();
    renderWithClient(<MemberProfileDialog user={userFixture} busy={false} />);

    await u.click(screen.getByRole("button", { name: /edit/i }));
    const emailInput = await screen.findByLabelText(/email/i);
    await u.tripleClick(emailInput);
    await u.type(emailInput, "new@x.dev");
    await u.click(screen.getByRole("button", { name: /save changes/i }));
    await screen.findByText(/already belongs to another account/i);

    await u.click(screen.getByRole("button", { name: /cancel/i }));
    await waitFor(() =>
      expect(
        screen.queryByText(/already belongs to another account/i),
      ).not.toBeInTheDocument(),
    );

    await u.click(screen.getByRole("button", { name: /edit/i }));
    expect(await screen.findByLabelText(/name/i)).toHaveValue("Ada");
    expect(screen.getByLabelText(/email/i)).toHaveValue("ada@x.dev");
    expect(
      screen.queryByText(/already belongs to another account/i),
    ).not.toBeInTheDocument();
  });
});
