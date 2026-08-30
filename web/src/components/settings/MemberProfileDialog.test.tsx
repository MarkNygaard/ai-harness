import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { AuthUser } from "@/lib/auth";
import { MemberProfileDialog } from "./MemberProfileDialog";

const USER: AuthUser = {
  id: "u1",
  name: "Ada Lovelace",
  email: "ada@example.com",
  role: "member",
  created_at: "2024-01-01T00:00:00Z",
  last_login_at: null,
  disabled_at: null,
};

function renderWithClient(ui: React.ReactElement) {
  const client = new QueryClient();
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

  it("prefills the dialog with the member's current name and email", async () => {
    renderWithClient(<MemberProfileDialog user={USER} disabled={false} />);

    fireEvent.click(screen.getByText("Edit"));

    const nameInput = (await screen.findByLabelText(
      /name/i,
    )) as HTMLInputElement;
    const emailInput = (await screen.findByLabelText(
      /email/i,
    )) as HTMLInputElement;

    expect(nameInput.value).toBe("Ada Lovelace");
    expect(emailInput.value).toBe("ada@example.com");
  });

  it("saves to PUT /api/users/{id} with a trimmed {name, email} body and closes", async () => {
    const mock = vi
      .fn()
      .mockResolvedValue(
        new Response(JSON.stringify({ ...USER, email: "grace@example.com" }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
      );
    global.fetch = mock as unknown as typeof fetch;

    renderWithClient(<MemberProfileDialog user={USER} disabled={false} />);

    fireEvent.click(screen.getByText("Edit"));
    const emailInput = (await screen.findByLabelText(
      /email/i,
    )) as HTMLInputElement;
    fireEvent.change(emailInput, {
      target: { value: "  grace@example.com  " },
    });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => expect(mock).toHaveBeenCalledOnce());
    const [path, init] = mock.mock.calls[0] as [string, RequestInit];
    expect(path).toBe("/api/users/u1");
    expect(init.method).toBe("PUT");
    expect(JSON.parse(init.body as string)).toEqual({
      name: "Ada Lovelace",
      email: "grace@example.com",
    });

    await waitFor(() => expect(screen.queryByLabelText(/email/i)).toBeNull());
  });

  it("shows the server's 409 sentence and keeps the dialog open", async () => {
    global.fetch = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          error: "taken@example.com already has an account here",
        }),
        { status: 409, headers: { "Content-Type": "application/json" } },
      ),
    ) as unknown as typeof fetch;

    renderWithClient(<MemberProfileDialog user={USER} disabled={false} />);

    fireEvent.click(screen.getByText("Edit"));
    const emailInput = (await screen.findByLabelText(
      /email/i,
    )) as HTMLInputElement;
    fireEvent.change(emailInput, { target: { value: "taken@example.com" } });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    expect(
      await screen.findByText(/already has an account here/i),
    ).toBeInTheDocument();
    expect(screen.getByLabelText(/email/i)).toBeInTheDocument();
    expect(screen.queryByText(/409/)).toBeNull();
  });

  it("disables save while a required field is empty", async () => {
    renderWithClient(<MemberProfileDialog user={USER} disabled={false} />);

    fireEvent.click(screen.getByText("Edit"));
    const nameInput = (await screen.findByLabelText(
      /name/i,
    )) as HTMLInputElement;
    const saveButton = screen.getByRole("button", { name: /save/i });

    expect(saveButton).toBeEnabled();

    fireEvent.change(nameInput, { target: { value: "" } });
    expect(saveButton).toBeDisabled();

    fireEvent.change(nameInput, { target: { value: "Ada Lovelace" } });
    expect(saveButton).toBeEnabled();
  });

  it("clears stale errors and re-seeds fields when reopened", async () => {
    global.fetch = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          error: "taken@example.com already has an account here",
        }),
        { status: 409, headers: { "Content-Type": "application/json" } },
      ),
    ) as unknown as typeof fetch;

    renderWithClient(<MemberProfileDialog user={USER} disabled={false} />);

    fireEvent.click(screen.getByText("Edit"));
    const emailInput = (await screen.findByLabelText(
      /email/i,
    )) as HTMLInputElement;
    fireEvent.change(emailInput, { target: { value: "taken@example.com" } });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    await screen.findByText(/already has an account here/i);

    fireEvent.click(screen.getByRole("button", { name: /close/i }));
    await waitFor(() => expect(screen.queryByLabelText(/email/i)).toBeNull());

    fireEvent.click(screen.getByText("Edit"));
    const reopenedName = (await screen.findByLabelText(
      /name/i,
    )) as HTMLInputElement;
    const reopenedEmail = (await screen.findByLabelText(
      /email/i,
    )) as HTMLInputElement;

    expect(reopenedName.value).toBe("Ada Lovelace");
    expect(reopenedEmail.value).toBe("ada@example.com");
    expect(screen.queryByText(/already has an account here/i)).toBeNull();
  });
});
