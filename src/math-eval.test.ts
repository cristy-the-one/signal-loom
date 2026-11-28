import { describe, expect, it } from "vitest";
import { MATH_RECIPES, parseMath, tokenAtCursor } from "./math-eval";
import type { SignalInfo } from "./types";

function sig(name: string, unit = ""): SignalInfo {
  return {
    name,
    unit,
    messageName: "ECM",
    messageId: 0x100,
    min: 0,
    max: 100,
    step: 0.1,
    fromMap: true,
  };
}

describe("parseMath", () => {
  it("evaluates arithmetic, power, comparisons, logic, and functions", () => {
    const vals = new Map([
      ["A", 10],
      ["B", 4],
      ["Speed", 0],
      ["Torque", 120],
    ]);

    const p1 = parseMath("2 ^ 3 ^ 2");
    expect(p1.ok).toBe(true);
    if (p1.ok) expect(p1.evaluate(vals)).toBe(512);

    const p2 = parseMath("if(Speed > 0, Torque / Speed, 0)");
    expect(p2.ok).toBe(true);
    if (p2.ok) {
      expect(p2.deps).toEqual(["Speed", "Torque"]);
      expect(p2.evaluate(vals)).toBe(0);
    }

    const p3 = parseMath("hypot(3, 4) + clamp(A, 0, 8) + avg(A, B) + min(A, B) + max(A, B)");
    expect(p3.ok).toBe(true);
    if (p3.ok) expect(p3.evaluate(vals)).toBe(5 + 8 + 7 + 4 + 10);

    const p4 = parseMath("A > 5 && B <= 4 && !(Speed != 0)");
    expect(p4.ok).toBe(true);
    if (p4.ok) expect(p4.evaluate(vals)).toBe(1);
  });

  it("detects stateful filters and validates parameters", () => {
    const lp = parseMath("lp(EngineRPM, 0.2)");
    expect(lp.ok).toBe(true);
    if (lp.ok) {
      expect(lp.stateful).toBe(true);
      expect(lp.deps).toEqual(["EngineRPM"]);
      expect(lp.evaluate(new Map([["EngineRPM", 2400]]))).toBe(2400);
    }

    const badLp = parseMath("lp(EngineRPM, 1.5)");
    expect(badLp.ok).toBe(false);

    const sma = parseMath("sma(WheelFL, 10)");
    expect(sma.ok).toBe(true);
    if (sma.ok) expect(sma.stateful).toBe(true);

    expect(parseMath("sma(WheelFL, 2.5)").ok).toBe(false);
    expect(parseMath("deriv(VehicleSpeed)").ok).toBe(true);
    expect(parseMath("integ(Power)").ok).toBe(true);
    expect(parseMath("delta(Gear)").ok).toBe(true);
    expect(parseMath("unknownFn(A)").ok).toBe(false);
  });

  it("extracts identifier tokens around cursor for autocomplete", () => {
    expect(tokenAtCursor("abs(Eng", 7)).toEqual({ token: "Eng", start: 4, end: 7 });
    expect(tokenAtCursor("WheelFL - WheelFR", 5)).toEqual({ token: "Wheel", start: 0, end: 7 });
    expect(tokenAtCursor("A + ", 4)).toEqual({ token: "", start: 4, end: 4 });
  });
});

describe("MATH_RECIPES", () => {
  it("builds valid expressions for every recipe", () => {
    const a = sig("WheelFL", "km/h");
    const b = sig("WheelFR", "km/h");
    for (const recipe of MATH_RECIPES) {
      const params: Record<string, string> = {};
      for (const p of recipe.params) params[p.key] = p.defaultValue;
      const built = recipe.build({ a, b }, params);
      expect(built, recipe.id).not.toBeNull();
      const parsed = parseMath(built!.expr);
      expect(parsed.ok, `${recipe.id}: ${built!.expr}`).toBe(true);
    }
  });
});
