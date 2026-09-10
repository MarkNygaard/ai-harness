/**
 * Key used to persist the bearer token. Stored in `localStorage` so it survives
 * across browser sessions — you enter it once, not on every open.
 */
export const TOKEN_KEY = "harness_token";

/**
 * EventTarget that dispatches a single "unauthorized" event type when the
 * server returns 401. The app mounts a listener in TokenPrompt.
 */
export const unauthorizedEvents = new EventTarget();

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    message: string,
    /**
     * The parsed error body, when the server sent one.
     *
     * A failure is not always just a sentence: an install refused for a name
     * collision also says which name and suggests a free one, and a caller that
     * only had the message would have to parse prose to find them. Carried as
     * `unknown` because it is whatever that route chose to send — the caller
     * checks the shape it expects.
     */
    public readonly body?: unknown,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

function authHeaders(): Record<string, string> {
  const tok = (globalThis.localStorage?.getItem?.(TOKEN_KEY) ?? "").trim();
  return tok ? { Authorization: `Bearer ${tok}` } : {};
}

export async function apiFetch(
  path: string,
  init: RequestInit = {},
): Promise<Response> {
  const merged: RequestInit = {
    ...init,
    headers: {
      Accept: "application/json",
      ...authHeaders(),
      ...(init.headers ?? {}),
    },
  };
  const resp = await fetch(path, merged);
  if (resp.status === 401) {
    unauthorizedEvents.dispatchEvent(new Event("unauthorized"));
    const { message, body } = await errorDetails(resp, path);
    throw new ApiError(401, message, body);
  }
  if (!resp.ok) {
    const { message, body } = await errorDetails(resp, path);
    throw new ApiError(resp.status, message, body);
  }
  return resp;
}

/**
 * What the server said went wrong, when it said anything.
 *
 * Routes report failures as `{"error": "..."}`, and those messages are written
 * for whoever is configuring the thing: "the mail server refused it:
 * certificate verify failed" is a problem you can go and fix, where
 * `HTTP 502` is only a shrug. Discarding the body meant every such message in
 * the app was written and then thrown away at the door.
 *
 * Falls back to the status line for anything that is not one of ours — a
 * proxy's HTML error page, an empty body, a truncated response.
 */
async function errorDetails(
  resp: Response,
  path: string,
): Promise<{ message: string; body?: unknown }> {
  const fallback = `${path} → HTTP ${resp.status}`;
  try {
    const body: unknown = await resp.json();
    const message = (body as { error?: unknown })?.error;
    return {
      message:
        typeof message === "string" && message.trim() ? message : fallback,
      body,
    };
  } catch {
    return { message: fallback };
  }
}

export async function apiJson<T>(path: string, init?: RequestInit): Promise<T> {
  const resp = await apiFetch(path, init);
  return (await resp.json()) as T;
}
