import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { CliUpdateButton } from "./parts";
import type { CliUpdateStatus } from "@/lib/system";

const IDLE: CliUpdateStatus = {
  active_runs: 0,
  installing: false,
  pending: [],
  completed: [],
  error: null,
};

/**
 * Record what was posted, and answer the update route the way the server would.
 *
 * What the button *says* is the whole feature — an admin who clicks "Update to
 * 2.1.4" and silently gets a queued install has been misled — so these assert
 * the label, not just that a request went out.
 */
function serve() {
  const posted: string[] = [];
  global.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const path = String(input);
    if (init?.method) posted.push(`${init.method} ${path}`);
    return new Response(JSON.stringify(IDLE));
  }) as unknown as typeof fetch;
  return posted;
}

function mount(queue: CliUpdateStatus) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <CliUpdateButton
        provider="claude"
        label="Claude Code"
        to="2.1.4"
        queue={queue}
      />
    </QueryClientProvider>,
  );
}

describe("<CliUpdateButton>", () => {
  const originalFetch = global.fetch;

  afterEach(() => {
    global.fetch = originalFetch;
    vi.restoreAllMocks();
  });

  it("offers the install outright when nothing is running", async () => {
    const posted = serve();
    mount(IDLE);

    const button = screen.getByRole("button", { name: /update to 2\.1\.4/i });
    await userEvent.click(button);

    await waitFor(() =>
      expect(posted).toContain("POST /api/system/cli-update/claude"),
    );
  });

  it("says it will wait, rather than promising an install, while runs are live", async () => {
    serve();
    mount({ ...IDLE, active_runs: 2 });

    const button = screen.getByRole("button", { name: /update when idle/i });
    // The count is the reason, so it is in reach of whoever wonders why.
    expect(button).toHaveAttribute(
      "title",
      expect.stringContaining("2 runs in flight"),
    );
    // Still pressable: queueing is the action, and refusing the click would
    // just send someone back later to guess when the cluster went quiet.
    expect(button).not.toBeDisabled();
  });

  it("shows a queued update as queued, with a way to take it back", async () => {
    const posted = serve();
    mount({ ...IDLE, active_runs: 1, pending: ["claude"] });

    expect(
      screen.getByText(/queued — waiting for 1 run to finish/i),
    ).toBeInTheDocument();
    // No install button while one is queued — there is nothing left to ask for.
    expect(
      screen.queryByRole("button", { name: /update/i }),
    ).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /cancel/i }));
    await waitFor(() =>
      expect(posted).toContain("DELETE /api/system/cli-update/claude"),
    );
  });

  it("cannot be cancelled once the install has started", () => {
    serve();
    mount({ ...IDLE, installing: true, pending: ["claude"] });

    expect(screen.getByText(/queued — installing now/i)).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /cancel/i }),
    ).not.toBeInTheDocument();
  });
});
