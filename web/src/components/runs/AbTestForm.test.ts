import { describe, expect, it } from "vitest";
import { baselineRefusal } from "./AbTestForm";

const connected = (
  ...providers: { id: string; models: string[] }[]
): { id: string; label: string; models: string[] }[] =>
  providers.map((p) => ({ ...p, label: p.id }));

const PI_KIMI_ONLY = connected({
  id: "pi",
  models: ["kimi-code/kimi-for-coding", "kimi-code/kimi-k2"],
});
const PI_BOTH = connected({
  id: "pi",
  models: ["openai-codex/gpt-5.5", "kimi-code/kimi-for-coding"],
});

describe("the baseline arm", () => {
  it("is refused when nothing backs its namespace", () => {
    // The case this exists for: arm B is picked from the gated catalog so it
    // always runs, while arm A comes from the workflow YAML unchecked. Left
    // alone, arm A fails, arm B succeeds, and that reads as a result.
    const refusal = baselineRefusal(
      { provider: "pi", model: "openai-codex/gpt-5.5" },
      PI_KIMI_ONLY,
    );
    expect(refusal).toContain("openai-codex/");
  });

  it("is refused when its agent has no account at all", () => {
    expect(
      baselineRefusal({ provider: "cursor", model: "composer" }, PI_BOTH),
    ).toContain("cursor");
  });

  it("allows a namespace that is connected", () => {
    expect(
      baselineRefusal(
        { provider: "pi", model: "kimi-code/kimi-for-coding" },
        PI_KIMI_ONLY,
      ),
    ).toBeNull();
  });

  it("allows a model the catalog does not list", () => {
    // The catalog's model lists are curated, not exhaustive — Cursor's entry
    // says any model string is accepted. Blocking on membership would refuse
    // workflows that run perfectly well, so only the namespace is checked.
    expect(
      baselineRefusal(
        { provider: "pi", model: "kimi-code/kimi-k9-unreleased" },
        PI_KIMI_ONLY,
      ),
    ).toBeNull();
  });

  it("allows a model with no namespace at all", () => {
    // `claude` models are bare ids (`opus`), so there is no namespace to check
    // and the provider being present is the whole answer.
    expect(
      baselineRefusal(
        { provider: "claude", model: "opus" },
        connected({ id: "claude", models: ["sonnet", "opus"] }),
      ),
    ).toBeNull();
  });

  it("says nothing while the catalog is still loading", () => {
    // An empty catalog is "not known yet", not "nothing is connected" — a
    // refusal there would flash on every open.
    expect(
      baselineRefusal({ provider: "pi", model: "kimi-code/x" }, []),
    ).toBeNull();
  });
});
