import { describe, expect, it } from "vitest";

import { parseInitiator, sourceLabel } from "./initiator";

describe("parseInitiator", () => {
  it("reads a name and address written the way the server writes them", () => {
    const got = parseInitiator("Andrius Mickus <andrius@dilling.com>");
    expect(got).toEqual({
      label: "Andrius Mickus",
      email: "andrius@dilling.com",
      initials: "AM",
    });
  });

  it("takes the first and last name, not the middle one", () => {
    expect(parseInitiator("Mark Nygaard Leth <m@x.com>")?.initials).toBe("ML");
  });

  it("shows one letter for a single name rather than inventing a second", () => {
    expect(parseInitiator("Andrius")?.initials).toBe("A");
  });

  it("falls back to the address when Linear disclosed no name", () => {
    const got = parseInitiator("mark.nygaard@dilling.com");
    // Initials come from the local part's own separators, so this is `MN` —
    // the whole address would only ever give `M`.
    expect(got).toEqual({
      label: "mark.nygaard@dilling.com",
      email: "mark.nygaard@dilling.com",
      initials: "MN",
    });
  });

  it("reads a bracketed address with an empty name", () => {
    expect(parseInitiator(" <solo@x.com>")).toEqual({
      label: "solo@x.com",
      email: "solo@x.com",
      initials: "S",
    });
  });

  it("keeps a name that came without an address", () => {
    expect(parseInitiator("Andrius Mickus")).toEqual({
      label: "Andrius Mickus",
      email: null,
      initials: "AM",
    });
  });

  it("is nobody when there is nobody, so views render nothing", () => {
    expect(parseInitiator(null)).toBeNull();
    expect(parseInitiator(undefined)).toBeNull();
    expect(parseInitiator("")).toBeNull();
    expect(parseInitiator("   ")).toBeNull();
  });

  it("survives a malformed actor rather than throwing in a list row", () => {
    // No closing bracket.
    expect(parseInitiator("Ann <ann@x.com")?.email).toBe("ann@x.com");
    // Brackets with nothing in them: still a name.
    expect(parseInitiator("Ann <>")).toEqual({
      label: "Ann",
      email: null,
      initials: "A",
    });
  });

  it("does not cut a multi-byte first letter in half", () => {
    expect(parseInitiator("Ægir Þórsson <a@x.com>")?.initials).toBe("ÆÞ");
  });

  it("still shows something for a label with no letters in it", () => {
    expect(parseInitiator("?!")?.initials).toBe("?");
  });
});

describe("sourceLabel", () => {
  it("names each door a run can arrive through", () => {
    expect(sourceLabel("ui")).toBe("from the dashboard");
    expect(sourceLabel("mcp")).toBe("over MCP");
    expect(sourceLabel("linear-webhook")).toBe("delegated in Linear");
    expect(sourceLabel("linear-poller")).toBe("moved into a Linear column");
  });

  it("still names the coarse source on rows written before the split", () => {
    expect(sourceLabel("linear")).toBe("from Linear");
  });

  it("says nothing for a source it does not know", () => {
    expect(sourceLabel(null)).toBeNull();
    expect(sourceLabel("carrier-pigeon")).toBeNull();
  });
});
