import { describe, expect, it } from "vitest";
import { savedAgo } from "./VersionHistoryDialog";

describe("savedAgo", () => {
  it("reads at the scale that matters", () => {
    expect(savedAgo(0, 30)).toBe("just now");
    expect(savedAgo(0, 60)).toBe("1 minute ago");
    expect(savedAgo(0, 12 * 60)).toBe("12 minutes ago");
    expect(savedAgo(0, 3 * 3600)).toBe("3 hours ago");
    expect(savedAgo(0, 2 * 86_400)).toBe("2 days ago");
    // A clock a little behind the server's never reads as the future.
    expect(savedAgo(100, 50)).toBe("just now");
  });
});
