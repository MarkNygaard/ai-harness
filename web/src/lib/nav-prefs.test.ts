import { beforeEach, describe, expect, it, vi } from "vitest";

const STORAGE_KEY = "harness.nav.hidden";

/** The store reads storage once at import, so each case needs a fresh module. */
async function loadFresh(stored?: unknown) {
  vi.resetModules();
  window.localStorage.clear();
  if (stored !== undefined) {
    window.localStorage.setItem(
      STORAGE_KEY,
      typeof stored === "string" ? stored : JSON.stringify(stored),
    );
  }
  return import("./nav-prefs");
}

function persisted(): string[] {
  return JSON.parse(window.localStorage.getItem(STORAGE_KEY)!);
}

beforeEach(() => {
  window.localStorage.clear();
});

describe("nav preferences", () => {
  it("reads a stored set at import", async () => {
    const { toggleNavHidden } = await loadFresh(["/ab", "/runs"]);
    // Toggling one entry off leaves the other — which it could only do if both
    // were loaded from storage rather than starting empty.
    toggleNavHidden("/ab");
    expect(persisted()).toEqual(["/runs"]);
  });

  it("toggles an entry on and back off, and persists it", async () => {
    const { toggleNavHidden } = await loadFresh();

    toggleNavHidden("/runs");
    expect(persisted()).toEqual(["/runs"]);

    toggleNavHidden("/ab");
    expect(persisted()).toEqual(["/ab", "/runs"]);

    toggleNavHidden("/runs");
    expect(persisted()).toEqual(["/ab"]);
  });

  it("reset clears everything", async () => {
    const { resetNavHidden } = await loadFresh(["/runs", "/ab"]);
    resetNavHidden();
    expect(persisted()).toEqual([]);
  });

  it("ignores storage that isn't a list of strings", async () => {
    // A hand edit, or a shape an older build wrote.
    for (const junk of ['{"nope":1}', "not json", JSON.stringify([1, null])]) {
      const { toggleNavHidden } = await loadFresh(junk);
      toggleNavHidden("/runs");
      // Started from empty rather than carrying the junk forward.
      expect(persisted()).toEqual(["/runs"]);
    }
  });

  it("survives storage that throws at import", async () => {
    // A private window, or a browser set to block site data: `getItem` throws
    // rather than returning null, and importing the module must not blow up.
    const spy = vi
      .spyOn(Storage.prototype, "getItem")
      .mockImplementation(() => {
        throw new Error("blocked");
      });
    vi.resetModules();
    const mod = await import("./nav-prefs");
    spy.mockRestore();

    // It loaded, and works from an empty set.
    mod.toggleNavHidden("/runs");
    expect(persisted()).toEqual(["/runs"]);
  });
});
