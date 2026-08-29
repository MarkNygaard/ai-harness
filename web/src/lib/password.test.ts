import { describe, expect, it } from "vitest";
import { scorePassword } from "./password";

const MIN = 12;

describe("scorePassword", () => {
  it("says nothing about an empty field", () => {
    const s = scorePassword("", MIN);
    expect(s.strength).toBe("short");
    expect(s.label).toBe("");
    expect(s.hint).toBeNull();
  });

  it("reports too-short before anything else", () => {
    const s = scorePassword("Tr0ub4dor&3", MIN); // 11 characters
    expect(s.strength).toBe("short");
    expect(s.hint).toContain("12");
  });

  it("does not mistake length alone for strength", () => {
    // Both clear the minimum; neither costs anything to guess.
    for (const cheap of [
      "aaaaaaaaaaaaaa",
      "abcdefghijklmn",
      "qwertyuiopasdf",
    ]) {
      const s = scorePassword(cheap, MIN);
      expect(s.score, `${cheap} scored ${s.score}`).toBeLessThanOrEqual(1);
    }
  });

  it("rates a long passphrase highly", () => {
    const s = scorePassword("correct horse battery staple", MIN);
    expect(s.strength).toBe("strong");
    expect(s.hint).toBeNull();
  });

  it("recognises the obvious words however they are dressed up", () => {
    const s = scorePassword("MyPassword2026!", MIN);
    expect(s.score).toBeLessThanOrEqual(1);
    expect(s.hint).toContain("common word");
  });

  it("catches a password that is just the person's own details", () => {
    const identity = ["Mark Nygaard", "mark.nygaard@example.test"];
    const s = scorePassword("nygaard-2026-x", MIN, identity);
    expect(s.score).toBeLessThanOrEqual(1);
    expect(s.hint).toContain("your own name or email");

    // The same password is fine for somebody it says nothing about.
    expect(
      scorePassword("nygaard-2026-x", MIN, ["ada@example.test"]).score,
    ).toBeGreaterThan(1);
  });

  it("ignores identity fragments too short to mean anything", () => {
    // A three-letter name must not condemn every password containing it.
    const s = scorePassword("thunder valley moss", MIN, ["moe@example.test"]);
    expect(s.strength).toBe("strong");
  });

  it("climbs with real added entropy", () => {
    const ladder = [
      "trombone1234",
      "trombone-vault-9",
      "trombone vault ridge 9",
      "trombone vault ridge lantern 9",
    ].map((p) => scorePassword(p, MIN).score);
    for (let i = 1; i < ladder.length; i += 1) {
      expect(ladder[i], `step ${i}: ${ladder}`).toBeGreaterThanOrEqual(
        ladder[i - 1],
      );
    }
    expect(ladder.at(-1)).toBe(4);
  });

  it("only withholds a hint once there is nothing to suggest", () => {
    expect(scorePassword("aaaaaaaaaaaaaa", MIN).hint).not.toBeNull();
    expect(scorePassword("correct horse battery staple", MIN).hint).toBeNull();
  });
});
