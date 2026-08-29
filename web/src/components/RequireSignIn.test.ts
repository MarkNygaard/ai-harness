import { describe, expect, it } from "vitest";
import { isPublicPath } from "./RequireSignIn";

describe("isPublicPath", () => {
  it("lets through the links sent to people who have no account", () => {
    // The bug this exists for: an invitation and a password reset are sent
    // precisely to someone who cannot sign in, so gating them behind sign-in
    // makes them impossible to use.
    expect(isPublicPath("/invite/9f3c1a2b4d5e6f70")).toBe(true);
    expect(isPublicPath("/forgot")).toBe(true);
  });

  it("lets through the ways in", () => {
    expect(isPublicPath("/login")).toBe(true);
    expect(isPublicPath("/setup")).toBe(true);
  });

  it("gates everything else", () => {
    for (const path of [
      "/",
      "/runs",
      "/settings/credentials",
      "/settings/members",
      "/editor/new",
      "/reports/geo-audit",
    ]) {
      expect(isPublicPath(path), path).toBe(false);
    }
  });

  it("does not open a page just because it starts like a public one", () => {
    // `startsWith` is only right for the path that carries a token segment.
    expect(isPublicPath("/invite")).toBe(false);
    expect(isPublicPath("/invites")).toBe(false);
    expect(isPublicPath("/login/secrets")).toBe(false);
    expect(isPublicPath("/setup-wizard")).toBe(false);
    expect(isPublicPath("/forgotten")).toBe(false);
  });
});
