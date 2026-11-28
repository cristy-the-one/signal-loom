import { describe, expect, it } from "vitest";
import { computeLaneLoupeWindows, formatHover, heldValue } from "./plot";

const points = [
  { t: 10, v: 1 },
  { t: 20, v: 2 },
  { t: 30, v: 3 },
];

describe("heldValue", () => {
  it("holds the latest sample at or before t", () => {
    expect(heldValue([], 10)).toBeNull();
    expect(heldValue(points, 9)).toBeNull();
    expect(heldValue(points, 10)).toBe(1);
    expect(heldValue(points, 25)).toBe(2);
    expect(heldValue(points, 100)).toBe(3);
    expect(
      heldValue(
        [
          { t: 10, v: 1 },
          { t: 10, v: 4 },
          { t: 20, v: 2 },
        ],
        10,
      ),
    ).toBe(4);
  });
});

describe("formatHover", () => {
  it("pads the reading and appends a unit when there is one", () => {
    const readout = { decimals: 1, width: 4 };
    expect(formatHover("Speed", 1.2, "km/h", readout)).toBe("Speed   1.2 km/h");
    expect(formatHover("Speed", 1.2, "", readout)).toBe("Speed   1.2");
    expect(formatHover("Speed", null, "km/h", readout)).toBe("Speed     — km/h");
  });

  it("uses the readout, including its negative-zero collapse, and appends enum labels", () => {
    expect(formatHover("N", -0.0001, "", { decimals: 3, width: 6 })).toBe("N   0.000");
    expect(formatHover("Gear", 3, "", { decimals: 0, width: 2 }, "3rd")).toBe("Gear   3 (3rd)");
  });
});

describe("computeLaneLoupeWindows", () => {
  const domain = { t0: 0, t1: 10_000_000 };

  it("produces 4x and 16x nested zoom-in windows centered on the pointer", () => {
    const [lvl1, lvl2] = computeLaneLoupeWindows({ t0: 1_000_000, t1: 2_000_000 }, 1_500_000, domain);
    expect(lvl1.factor).toBe(4);
    expect(lvl1.t1 - lvl1.t0).toBe(250_000);
    expect((lvl1.t0 + lvl1.t1) / 2).toBe(1_500_000);

    expect(lvl2.factor).toBe(16);
    expect(lvl2.t1 - lvl2.t0).toBe(62_500);
    expect(lvl2.t0).toBeGreaterThan(lvl1.t0);
    expect(lvl2.t1).toBeLessThan(lvl1.t1);
  });

  it("still magnifies 2 levels deeper even at normal 1 ms max zoom", () => {
    const [lvl1, lvl2] = computeLaneLoupeWindows({ t0: 500_000, t1: 501_000 }, 500_500, domain);
    expect(lvl1.t1 - lvl1.t0).toBe(250);
    expect(lvl2.t1 - lvl2.t0).toBe(62.5);
    expect(lvl1.factor).toBe(4);
    expect(lvl2.factor).toBe(16);
  });

  it("clamps cleanly to recording start and end boundaries", () => {
    const [lvl1, lvl2] = computeLaneLoupeWindows({ t0: 0, t1: 1_000_000 }, 10_000, domain);
    expect(lvl1.t0).toBe(0);
    expect(lvl1.t1).toBe(250_000);
    expect(lvl2.t0).toBe(0);
    expect(lvl2.t1).toBe(62_500);
  });
});
