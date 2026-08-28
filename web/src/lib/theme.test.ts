import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  applyTheme,
  readThemePreference,
  resolveTheme,
  THEME_STORAGE_KEY,
} from "./theme";

/** Stand in for the OS preference, which jsdom doesn't provide at all. */
function mockSystemDark(matches: boolean) {
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    writable: true,
    value: vi.fn().mockReturnValue({
      matches,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    }),
  });
}

beforeEach(() => {
  window.localStorage.clear();
  document.documentElement.classList.remove("dark");
  mockSystemDark(false);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("readThemePreference", () => {
  it("defaults to following the system", () => {
    expect(readThemePreference()).toBe("system");
  });

  it("reads a stored choice back", () => {
    for (const stored of ["light", "dark", "system"] as const) {
      window.localStorage.setItem(THEME_STORAGE_KEY, stored);
      expect(readThemePreference()).toBe(stored);
    }
  });

  it("ignores a value that isn't a preference", () => {
    // Hand-edited storage, or a key left by an older build.
    window.localStorage.setItem(THEME_STORAGE_KEY, "solarized");
    expect(readThemePreference()).toBe("system");
  });

  it("falls back to system when storage throws", () => {
    // A private window or a browser set to block site data doesn't return
    // null — it throws on access.
    vi.spyOn(window.localStorage, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(readThemePreference()).toBe("system");
  });
});

describe("resolveTheme", () => {
  it("pins light and dark regardless of the system", () => {
    mockSystemDark(true);
    expect(resolveTheme("light")).toBe("light");
    expect(resolveTheme("dark")).toBe("dark");
    mockSystemDark(false);
    expect(resolveTheme("light")).toBe("light");
    expect(resolveTheme("dark")).toBe("dark");
  });

  it("follows the system when set to system", () => {
    mockSystemDark(true);
    expect(resolveTheme("system")).toBe("dark");
    mockSystemDark(false);
    expect(resolveTheme("system")).toBe("light");
  });

  it("resolves to light where the system can't be asked", () => {
    // `matchMedia` is missing in some embedded webviews; the page must still
    // render one theme rather than crashing.
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      writable: true,
      value: undefined,
    });
    expect(resolveTheme("system")).toBe("light");
  });
});

describe("applyTheme", () => {
  it("adds and removes the class the palette keys off", () => {
    applyTheme("dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    applyTheme("light");
    expect(document.documentElement.classList.contains("dark")).toBe(false);
  });

  it("is idempotent", () => {
    applyTheme("dark");
    applyTheme("dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
  });
});
