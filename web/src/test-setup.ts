import "@testing-library/jest-dom/vitest";

function createMemoryStorage(): Storage {
  const values = new Map<string, string>();
  return {
    get length() {
      return values.size;
    },
    clear() {
      values.clear();
    },
    getItem(key: string) {
      return values.get(key) ?? null;
    },
    key(index: number) {
      return Array.from(values.keys())[index] ?? null;
    },
    removeItem(key: string) {
      values.delete(key);
    },
    setItem(key: string, value: string) {
      values.set(key, value);
    },
  };
}


function setupGlobalStorage(name: "localStorage" | "sessionStorage") {
  if (typeof window === "undefined") return;
  try {
    const existing = (globalThis as Record<string, unknown>)[name] as Storage | undefined;
    const hasUsableStorage =
      typeof existing?.getItem === "function" &&
      typeof existing?.setItem === "function" &&
      typeof existing?.clear === "function";
    if (!hasUsableStorage) {
      const storage = createMemoryStorage();
      Object.defineProperty(globalThis, name, { value: storage, configurable: true });
      Object.defineProperty(window, name, { value: storage, configurable: true });
    }
  } catch {
    // ignore
  }
}

setupGlobalStorage("localStorage");
setupGlobalStorage("sessionStorage");
