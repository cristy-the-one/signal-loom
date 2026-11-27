import { describe, expect, it } from "vitest";
import { formatHover, heldValue } from "./plot";

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
