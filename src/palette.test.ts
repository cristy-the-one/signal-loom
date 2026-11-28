import { describe, expect, it } from "vitest";
import { fuzzyMatch } from "./fuzzy";

describe("fuzzyMatch", () => {
  it("matches subsequences case-insensitively and returns highlight indices", () => {
    expect(fuzzyMatch("", "Zoom in")).toEqual({ score: 0, indices: [] });
    const hit = fuzzyMatch("zm", "Zoom in");
    expect(hit).not.toBeNull();
    expect(hit!.indices).toEqual([0, 3]);
    expect(fuzzyMatch("xyz", "Zoom in")).toBeNull();
  });

  it("ranks prefix and word-boundary matches higher than scattered matches", () => {
    const exact = fuzzyMatch("zoom", "Zoom to fit entire log")!;
    const scattered = fuzzyMatch("zoom", "Freeze tool monitor")!;
    expect(exact).not.toBeNull();
    expect(scattered).not.toBeNull();
    expect(exact.score).toBeGreaterThan(scattered.score);

    const boundary = fuzzyMatch("sb", "Signal bar")!;
    const mid = fuzzyMatch("sb", "Assembling")!;
    expect(boundary.score).toBeGreaterThan(mid.score);
  });
});
