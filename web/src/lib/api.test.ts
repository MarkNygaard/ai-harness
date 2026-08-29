import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { apiFetch, TOKEN_KEY, unauthorizedEvents } from "./api";

describe("apiFetch", () => {
  const originalFetch = global.fetch;

  beforeEach(() => {
    localStorage.clear();
  });

  afterEach(() => {
    global.fetch = originalFetch;
    vi.restoreAllMocks();
  });

  it("injects Authorization header when token is set", async () => {
    localStorage.setItem(TOKEN_KEY, "abc123");
    const mock = vi.fn().mockResolvedValue(new Response("{}", { status: 200 }));
    global.fetch = mock as unknown as typeof fetch;

    await apiFetch("/api/overview");

    expect(mock).toHaveBeenCalledOnce();
    const [, init] = mock.mock.calls[0];
    const headers = new Headers((init as RequestInit).headers);
    expect(headers.get("Authorization")).toBe("Bearer abc123");
  });

  it("omits Authorization when no token", async () => {
    const mock = vi.fn().mockResolvedValue(new Response("{}", { status: 200 }));
    global.fetch = mock as unknown as typeof fetch;

    await apiFetch("/api/overview");

    const [, init] = mock.mock.calls[0];
    const headers = new Headers((init as RequestInit).headers);
    expect(headers.get("Authorization")).toBeNull();
  });

  it("dispatches unauthorized event on 401 and throws", async () => {
    global.fetch = vi
      .fn()
      .mockResolvedValue(
        new Response("{}", { status: 401 }),
      ) as unknown as typeof fetch;

    const handler = vi.fn();
    unauthorizedEvents.addEventListener("unauthorized", handler);

    await expect(apiFetch("/api/overview")).rejects.toThrow(/401/);
    expect(handler).toHaveBeenCalledOnce();

    unauthorizedEvents.removeEventListener("unauthorized", handler);
  });

  it("surfaces the server's message instead of the status code", async () => {
    // The case this exists for: a 502 from the mail test carries the only
    // sentence that says what to change.
    global.fetch = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          error: "the mail server refused it: certificate verify failed",
        }),
        { status: 502 },
      ),
    ) as unknown as typeof fetch;

    await expect(apiFetch("/api/settings/mail/test")).rejects.toThrow(
      "the mail server refused it: certificate verify failed",
    );
  });

  it("falls back to the status line when the body is not ours", async () => {
    // A proxy's HTML error page, an empty body, a blank message.
    for (const body of ["<html>502 Bad Gateway</html>", "", '{"error":"  "}']) {
      global.fetch = vi
        .fn()
        .mockResolvedValue(
          new Response(body, { status: 502 }),
        ) as unknown as typeof fetch;

      await expect(apiFetch("/api/overview")).rejects.toThrow(/HTTP 502/);
    }
  });

  it("reports the status for an error with no body at all", async () => {
    global.fetch = vi
      .fn()
      .mockResolvedValue(
        new Response(null, { status: 503 }),
      ) as unknown as typeof fetch;

    await expect(apiFetch("/api/overview")).rejects.toThrow(/HTTP 503/);
  });
});
