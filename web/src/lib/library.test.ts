import { describe, expect, it } from "vitest";

import { ApiError } from "./api";
import { InstallNameConflict, asConflict, installRequestInit } from "./library";

describe("asConflict", () => {
  /**
   * A name collision is a question for the person — which name would you like?
   * — and every other failure is not. Telling them apart is what decides
   * whether the dialog shows a prompt or an error, so it is read from the
   * body's shape rather than from the status code alone.
   */
  it("recognises a name conflict and carries the suggestion", () => {
    const e = asConflict(
      new ApiError(409, "this harness already has a workflow called `mine`", {
        error: "this harness already has a workflow called `mine`",
        conflict: "mine",
        suggested_name: "mine-2",
      }),
    );
    expect(e).toBeInstanceOf(InstallNameConflict);
    expect((e as InstallNameConflict).conflict).toEqual({
      conflict: "mine",
      suggested_name: "mine-2",
    });
  });

  /**
   * Every suffixed name can be taken too. The prompt still has to open — the
   * person can type a name of their own — so a missing suggestion is null
   * rather than a reason to fall back to a plain error.
   */
  it("survives a conflict with no suggestion left to make", () => {
    const e = asConflict(
      new ApiError(409, "taken", { conflict: "mine", suggested_name: null }),
    );
    expect(e).toBeInstanceOf(InstallNameConflict);
    expect((e as InstallNameConflict).conflict.suggested_name).toBeNull();
  });

  /** A registry that is down is not a question. It must stay a plain error. */
  it("leaves other failures alone", () => {
    const down = new ApiError(502, "could not reach the workflow library");
    expect(asConflict(down)).toBe(down);
    expect(asConflict(down)).not.toBeInstanceOf(InstallNameConflict);
  });

  /**
   * The body is whatever that route chose to send, so the shape is checked
   * rather than assumed: throwing a TypeError while inspecting an error would
   * replace a message the person could act on with one nobody can.
   */
  it("does not trust the shape of what it was given", () => {
    for (const body of [
      undefined,
      null,
      "a string",
      42,
      { conflict: 12 },
      { suggested_name: "mine-2" },
      {},
    ]) {
      const e = asConflict(new ApiError(409, "odd", body));
      expect(e).not.toBeInstanceOf(InstallNameConflict);
      expect(e).toBeInstanceOf(Error);
    }
    // Not an Error at all — still has to come back as one.
    expect(asConflict("just a string")).toBeInstanceOf(Error);
    expect(asConflict(undefined)).toBeInstanceOf(Error);
  });
});

describe("installRequestInit", () => {
  /**
   * The ordinary install has nothing to say, and the route's body is optional —
   * so it sends none. An empty `{}` is not harmless: without a
   * `Content-Type: application/json` header the server refuses the request with
   * a `415`, which is how this shipped broken.
   */
  it("sends no body when there is nothing to ask for", () => {
    const init = installRequestInit();
    expect(init.method).toBe("POST");
    expect(init.body).toBeUndefined();
    expect(init.headers).toBeUndefined();
  });

  /** A chosen name is JSON, and JSON has to say so. */
  it("declares the content type whenever it sends a body", () => {
    const init = installRequestInit("geo-audit-ecommerce");
    expect(init.body).toBe(JSON.stringify({ name: "geo-audit-ecommerce" }));
    expect(init.headers).toEqual({ "Content-Type": "application/json" });
  });

  /** An empty name is no name — it must not become a body either. */
  it("treats an empty name as no name", () => {
    expect(installRequestInit("").body).toBeUndefined();
  });
});
