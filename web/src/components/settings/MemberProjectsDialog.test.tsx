import * as React from "react";
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemberProjectsDialog } from "./MemberProjectsDialog";
import type { UserActivity } from "@/lib/users";

const member: UserActivity = {
  id: "u/1",
  email: "ada@x.dev",
  name: "Ada",
  role: "member",
  created_at: "2025-01-01T00:00:00Z",
  last_login_at: null,
  disabled_at: null,
  last_active_at: null,
  projects: [],
};

/** `GET /api/projects` answers with these; everything else echoes the body. */
function fetchWithProjects(names: string[]) {
  return vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.includes("/api/projects")) {
      return Promise.resolve(
        new Response(JSON.stringify(names.map((name) => ({ name })))),
      );
    }
    return Promise.resolve(new Response(init?.body?.toString() ?? "{}"));
  });
}

function renderWithClient(ui: React.ReactElement) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>{ui}</QueryClientProvider>,
  );
}

describe("<MemberProjectsDialog>", () => {
  const originalFetch = global.fetch;

  beforeEach(() => localStorage.clear());
  afterEach(() => {
    global.fetch = originalFetch;
    vi.restoreAllMocks();
  });

  it("opens on 'Every project' for a member nobody has limited", async () => {
    global.fetch = fetchWithProjects(["niles", "shop"]);
    const u = userEvent.setup();
    renderWithClient(<MemberProjectsDialog user={member} busy={false} />);

    await u.click(screen.getByRole("button", { name: /projects/i }));

    expect(
      await screen.findByRole("radio", { name: /every project/i }),
    ).toBeChecked();
    // The checklist is only meaningful once "only the ones I pick" is on.
    expect(screen.queryByRole("checkbox", { name: "niles" })).toBeNull();
  });

  it("opens pre-ticked for a member who is already limited", async () => {
    global.fetch = fetchWithProjects(["niles", "shop"]);
    const u = userEvent.setup();
    renderWithClient(
      <MemberProjectsDialog
        user={{ ...member, projects: ["niles"] }}
        busy={false}
      />,
    );

    await u.click(screen.getByRole("button", { name: /projects/i }));

    expect(
      await screen.findByRole("radio", { name: /only the projects i pick/i }),
    ).toBeChecked();
    expect(
      await screen.findByRole("checkbox", { name: "niles" }),
    ).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "shop" })).not.toBeChecked();
  });

  it("PUTs the picked projects", async () => {
    const mock = fetchWithProjects(["niles", "shop"]);
    global.fetch = mock;
    const u = userEvent.setup();
    renderWithClient(<MemberProjectsDialog user={member} busy={false} />);

    await u.click(screen.getByRole("button", { name: /projects/i }));
    await u.click(
      await screen.findByRole("radio", { name: /only the projects i pick/i }),
    );
    await u.click(await screen.findByRole("checkbox", { name: "shop" }));
    await u.click(screen.getByRole("button", { name: /save changes/i }));

    await waitFor(() => {
      const put = mock.mock.calls.find(
        ([, init]) => (init as RequestInit | undefined)?.method === "PUT",
      );
      expect(put).toBeTruthy();
      expect(String(put?.[0])).toContain("/api/users/u%2F1/projects");
      expect(JSON.parse(String((put?.[1] as RequestInit).body))).toEqual({
        projects: ["shop"],
      });
    });
  });

  it("sends an empty list to clear a limit, which means every project", async () => {
    const mock = fetchWithProjects(["niles"]);
    global.fetch = mock;
    const u = userEvent.setup();
    renderWithClient(
      <MemberProjectsDialog
        user={{ ...member, projects: ["niles"] }}
        busy={false}
      />,
    );

    await u.click(screen.getByRole("button", { name: /projects/i }));
    await u.click(await screen.findByRole("radio", { name: /every project/i }));
    await u.click(screen.getByRole("button", { name: /save changes/i }));

    await waitFor(() => {
      const put = mock.mock.calls.find(
        ([, init]) => (init as RequestInit | undefined)?.method === "PUT",
      );
      expect(JSON.parse(String((put?.[1] as RequestInit).body))).toEqual({
        projects: [],
      });
    });
  });

  it("will not save an empty pick, which would clear the limit instead", async () => {
    const mock = fetchWithProjects(["niles"]);
    global.fetch = mock;
    const u = userEvent.setup();
    renderWithClient(<MemberProjectsDialog user={member} busy={false} />);

    await u.click(screen.getByRole("button", { name: /projects/i }));
    await u.click(
      await screen.findByRole("radio", { name: /only the projects i pick/i }),
    );

    expect(
      screen.getByRole("button", { name: /save changes/i }),
    ).toBeDisabled();
    expect(screen.getByText(/pick at least one project/i)).toBeInTheDocument();
    expect(
      mock.mock.calls.some(
        ([, init]) => (init as RequestInit | undefined)?.method === "PUT",
      ),
    ).toBe(false);
  });

  it("keeps a project that is no longer registered rather than dropping it", async () => {
    // A scope can outlive its project; silently discarding it on the next save
    // would widen the member's access without anyone choosing that.
    global.fetch = fetchWithProjects(["niles"]);
    const u = userEvent.setup();
    renderWithClient(
      <MemberProjectsDialog
        user={{ ...member, projects: ["niles", "retired"] }}
        busy={false}
      />,
    );

    await u.click(screen.getByRole("button", { name: /projects/i }));

    expect(
      await screen.findByRole("checkbox", {
        name: /retired \(not registered\)/i,
      }),
    ).toBeChecked();
  });

  it("is unavailable for an administrator, who always sees everything", async () => {
    global.fetch = fetchWithProjects(["niles"]);
    renderWithClient(
      <MemberProjectsDialog user={{ ...member, role: "admin" }} busy={false} />,
    );

    expect(screen.getByRole("button", { name: /projects/i })).toBeDisabled();
  });
});
