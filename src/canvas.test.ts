import { describe, expect, it } from "vitest";
import { eachTick } from "./canvas";

function ticks(t0: number, t1: number): number[] {
  const out: number[] = [];
  eachTick(t0, t1, (t) => out.push(t));
  return out;
}

describe("eachTick", () => {
  it("visits every grid tick inside the span, including both ends when they land on the grid", () => {
    expect(ticks(0, 6_000)).toEqual([0, 1_000, 2_000, 3_000, 4_000, 5_000, 6_000]);
  });

  it("starts at the first tick on or after t0", () => {
    expect(ticks(600, 2_400)).toEqual([1_000, 1_500, 2_000]);
  });

  it("visits a zero-length span once when it sits on a tick, and visits nothing when reversed", () => {
    expect(ticks(1_000, 1_000)).toEqual([1_000]);
    expect(ticks(2_000, 1_000)).toEqual([]);
  });
});
