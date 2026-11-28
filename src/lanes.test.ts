import { describe, expect, it } from "vitest";
import { buildLaneSpecs, cycleSignalLane, isRibbonStyle, laneModeLabel, nextLaneMode, resolveTraceStyle } from "./lanes";
import type { SignalInfo } from "./types";

function sig(name: string, unit = "", messageName = "ECM"): SignalInfo {
  return {
    name,
    unit,
    messageName,
    messageId: 0x100,
    min: 0,
    max: 100,
    step: 0.1,
    fromMap: true,
  };
}

const catalog = new Map<string, SignalInfo>([
  ["WheelFL", sig("WheelFL", "km/h", "Wheels")],
  ["WheelFR", sig("WheelFR", "km/h", "Wheels")],
  ["EngineRPM", sig("EngineRPM", "rpm", "ECM")],
  ["BrakeLamp", sig("BrakeLamp", "", "Lamps")],
  ["AbsLamp", sig("AbsLamp", "", "Lamps")],
]);

const lookup = (name: string) => catalog.get(name);

describe("nextLaneMode & laneModeLabel", () => {
  it("cycles overlay -> split -> grouped -> overlay", () => {
    expect(nextLaneMode("overlay")).toBe("split");
    expect(nextLaneMode("split")).toBe("grouped");
    expect(nextLaneMode("grouped")).toBe("overlay");
    expect(laneModeLabel("overlay")).toBe("Overlay");
    expect(laneModeLabel("split")).toBe("Split");
    expect(laneModeLabel("grouped")).toBe("Group");
  });
});

describe("buildLaneSpecs", () => {
  it("puts all signals on one track in overlay mode", () => {
    expect(buildLaneSpecs([], "overlay", {}, lookup)).toEqual([]);
    const multi = buildLaneSpecs(["WheelFL", "EngineRPM"], "overlay", {}, lookup);
    expect(multi).toHaveLength(1);
    expect(multi[0].signals).toEqual(["WheelFL", "EngineRPM"]);
    expect(multi[0].sharedScale).toBe(false);

    const single = buildLaneSpecs(["WheelFL"], "overlay", {}, lookup);
    expect(single[0].sharedScale).toBe(true);
  });

  it("splits each signal onto its own track in split mode", () => {
    const specs = buildLaneSpecs(["WheelFL", "EngineRPM", "BrakeLamp"], "split", {}, lookup);
    expect(specs.map((s) => ({ badge: s.badge, title: s.title, signals: s.signals, shared: s.sharedScale }))).toEqual([
      { badge: "T1", title: "WheelFL (km/h)", signals: ["WheelFL"], shared: true },
      { badge: "T2", title: "EngineRPM (rpm)", signals: ["EngineRPM"], shared: true },
      { badge: "T3", title: "BrakeLamp", signals: ["BrakeLamp"], shared: true },
    ]);
  });

  it("auto-groups by unit and message in grouped mode with shared Y-scale", () => {
    const specs = buildLaneSpecs(
      ["WheelFL", "EngineRPM", "WheelFR", "BrakeLamp", "AbsLamp"],
      "grouped",
      {},
      lookup,
    );
    expect(specs).toHaveLength(3);
    expect(specs[0]).toMatchObject({
      badge: "T1",
      title: "T1 · km/h (2)",
      signals: ["WheelFL", "WheelFR"],
      sharedScale: true,
    });
    expect(specs[1]).toMatchObject({
      badge: "T2",
      title: "EngineRPM (rpm)",
      signals: ["EngineRPM"],
      sharedScale: true,
    });
    expect(specs[2]).toMatchObject({
      badge: "T3",
      title: "T3 · Lamps (2)",
      signals: ["BrakeLamp", "AbsLamp"],
      sharedScale: true,
    });
  });

  it("honours manual customLanes overrides and detects mixed-unit tracks", () => {
    const specs = buildLaneSpecs(
      ["WheelFL", "EngineRPM", "WheelFR"],
      "grouped",
      { WheelFL: 1, EngineRPM: 1 },
      lookup,
    );
    expect(specs).toHaveLength(2);
    expect(specs[0].signals).toEqual(["WheelFL", "EngineRPM"]);
    expect(specs[0].sharedScale).toBe(false);
    expect(specs[1].signals).toEqual(["WheelFR"]);
    expect(specs[1].sharedScale).toBe(true);
  });
});

describe("cycleSignalLane", () => {
  it("cycles through group numbers and resets back to auto", () => {
    const plotted = ["A", "B", "C"];
    const step1 = cycleSignalLane("A", plotted, {});
    expect(step1).toEqual({ A: 1 });
    const step2 = cycleSignalLane("A", plotted, step1);
    expect(step2).toEqual({ A: 2 });
    const step3 = cycleSignalLane("A", plotted, step2);
    expect(step3).toEqual({ A: 3 });
    const reset = cycleSignalLane("A", plotted, step3);
    expect(reset).toEqual({});
  });
});

describe("semantic trace styles & compact ribbon lanes", () => {
  it("auto-detects state ribbons for enums/small ints and flag ribbons for booleans", () => {
    const enumSig: SignalInfo = {
      ...sig("DriveMode", "", "Body"),
      min: 0,
      max: 3,
      step: 1,
      labels: [
        { value: 0, label: "ECO" },
        { value: 1, label: "SPORT" },
      ],
    };
    const boolSig: SignalInfo = { ...sig("AbsActive", "", "ABS"), min: 0, max: 1, step: 1 };
    const gearSig: SignalInfo = { ...sig("Gear", "", "Body"), min: 0, max: 6, step: 1 };
    const analogSig = sig("EngineRPM", "rpm", "ECM");

    expect(resolveTraceStyle(enumSig)).toBe("state");
    expect(resolveTraceStyle(boolSig)).toBe("flag");
    expect(resolveTraceStyle(gearSig)).toBe("state");
    expect(resolveTraceStyle(analogSig)).toBe("step");
    expect(resolveTraceStyle(enumSig, "step")).toBe("step");
    expect(resolveTraceStyle(analogSig, "area")).toBe("area");
    expect(isRibbonStyle("state")).toBe(true);
    expect(isRibbonStyle("flag")).toBe(true);
    expect(isRibbonStyle("heat")).toBe(true);
    expect(isRibbonStyle("area")).toBe(false);
  });

  it("gives enum/flag signals compact lanes below the analog overlay in overlay mode", () => {
    const map = new Map<string, SignalInfo>([
      ["EngineRPM", sig("EngineRPM", "rpm", "ECM")],
      ["VehicleSpeed", sig("VehicleSpeed", "km/h", "ECM")],
      ["Gear", { ...sig("Gear", "", "Body"), min: 0, max: 6, step: 1 }],
    ]);
    const specs = buildLaneSpecs(["EngineRPM", "Gear", "VehicleSpeed"], "overlay", {}, (n) => map.get(n));
    expect(specs).toHaveLength(2);
    expect(specs[0]).toMatchObject({ signals: ["EngineRPM", "VehicleSpeed"], compact: false });
    expect(specs[1]).toMatchObject({ signals: ["Gear"], compact: true });
  });
});
