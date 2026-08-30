import * as React from "react";
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useSetUserProfile } from "./users";
import type { AuthUser } from "./auth";

const user: AuthUser = {
  id: "u/1",
  email: "ada@x.dev",
  name: "Ada",
  role: "member",
  created_at: "2025-01-01T00:00:00Z",
  last_login_at: null,
  disabled_at: null,
};

describe("useSetUserProfile", () => {
  const originalFetch = global.fetch;

  beforeEach(() => {
    localStorage.clear();
  });

  afterEach(() => {
    global.fetch = originalFetch;
    vi.restoreAllMocks();
  });

  function wrapper({ children }: { children: React.ReactNode }) {
    const client = new QueryClient({
      defaultOptions: {
        queries: { retry: false },
        mutations: { retry: false },
      },
    });
    return React.createElement(QueryClientProvider, { client }, children);
  }

  it("PUTs the name and email to the encoded user URL", async () => {
    const mock = vi.fn().mockResolvedValue(new Response(JSON.stringify(user)));
    global.fetch = mock as unknown as typeof fetch;

    const { result } = renderHook(() => useSetUserProfile(), { wrapper });
    result.current.mutate({
      id: "u/1",
      name: "Ada",
      email: "ada@x.dev",
    });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));

    expect(mock).toHaveBeenCalledOnce();
    const [url, init] = mock.mock.calls[0];
    expect(url).toBe("/api/users/u%2F1");
    expect((init as RequestInit).method).toBe("PUT");
    const headers = new Headers((init as RequestInit).headers);
    expect(headers.get("Content-Type")).toBe("application/json");
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({
      name: "Ada",
      email: "ada@x.dev",
    });
  });

  it("surfaces the server's 409 duplicate-email sentence", async () => {
    // This is the case the feature exists for: the email address is unique.
    const mock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          error: "ada@x.dev already belongs to another account",
        }),
        { status: 409 },
      ),
    );
    global.fetch = mock as unknown as typeof fetch;

    const { result } = renderHook(() => useSetUserProfile(), { wrapper });
    result.current.mutate({
      id: "u/1",
      name: "Ada",
      email: "ada@x.dev",
    });

    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(result.current.error?.message).toBe(
      "ada@x.dev already belongs to another account",
    );
  });

  it("invalidates the users and auth/status queries on success", async () => {
    const mock = vi.fn().mockResolvedValue(new Response(JSON.stringify(user)));
    global.fetch = mock as unknown as typeof fetch;

    const client = new QueryClient({
      defaultOptions: {
        queries: { retry: false },
        mutations: { retry: false },
      },
    });
    const invalidateQueries = vi.spyOn(client, "invalidateQueries");

    function clientWrapper({ children }: { children: React.ReactNode }) {
      return React.createElement(QueryClientProvider, { client }, children);
    }

    const { result } = renderHook(() => useSetUserProfile(), {
      wrapper: clientWrapper,
    });
    result.current.mutate({ id: "u/1", name: "Ada", email: "ada@x.dev" });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));

    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["users"] });
    expect(invalidateQueries).toHaveBeenCalledWith({
      queryKey: ["auth", "status"],
    });
  });
});
