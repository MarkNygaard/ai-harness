import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { Provenance } from "./Provenance";

const CREATED = "2026-11-09T07:12:20Z";

describe("Provenance", () => {
  it("names who created it, and leaves out a change that never happened", () => {
    render(
      <Provenance
        row={{
          created_at: CREATED,
          updated_at: CREATED,
          created_by_actor: "Mark Nygaard <mark@x.dev>",
          updated_by_actor: "Mark Nygaard <mark@x.dev>",
        }}
      />,
    );
    expect(screen.getByText(/Created by Mark Nygaard/)).toBeTruthy();
    expect(screen.queryByText(/Last changed/)).toBeNull();
  });

  it("names who changed it last", () => {
    render(
      <Provenance
        row={{
          created_at: CREATED,
          updated_at: "2026-11-09T12:25:34Z",
          created_by_actor: "Mark Nygaard <mark@x.dev>",
          updated_by_actor: "Henrik <henrik@x.dev>",
        }}
      />,
    );
    expect(screen.getByText(/Last changed by Henrik/)).toBeTruthy();
  });

  it("says when without a name when nobody is recorded", () => {
    render(
      <Provenance
        row={{ created_at: CREATED, updated_at: "2026-11-09T12:25:34Z" }}
      />,
    );
    expect(screen.getByText(/^Created ·/)).toBeTruthy();
    expect(screen.getByText(/^Last changed ·/)).toBeTruthy();
  });
});
