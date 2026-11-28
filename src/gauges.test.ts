import { describe, expect, it } from "vitest";
import {
  defaultClusterWidgets,
  dropIndexAt,
  evalLamp,
  layoutWidgets,
  readingsFrom,
  resolveWidgets,
  serializeClusterWidgets,
  slotAt,
  slotSpec,
  smartWidgetForSignal,
  widgetAt,
} from "./gauges";
import type { ClusterWidget, SignalInfo, ValueRead } from "./types";

function signal(partial: Partial<SignalInfo> & Pick<SignalInfo, "name">): SignalInfo {
  return {
    unit: "",
    messageName: "",
    messageId: null,
    min: 0,
    max: 1,
    step: null,
    fromMap: true,
    ...partial,
  };
}

function reading(name: string, value: number): ValueRead {
  return { name, unit: "", value };
}

describe("slotSpec", () => {
  it("returns the slot's sample defaults", () => {
    expect(slotSpec("rpm").defaults).toEqual(["EngineRPM", "DisplayedRPM"]);
  });

  it("falls back to the speed slot for an unknown id", () => {
    expect(slotSpec("nope" as "rpm").id).toBe("speed");
  });
});

describe("readingsFrom", () => {
  it("uses an assigned signal's range and held value", () => {
    const speed = readingsFrom(
      [reading("VehicleSpeed", 88)],
      [signal({ name: "VehicleSpeed", unit: "km/h", min: 0, max: 180 })],
      { speed: "VehicleSpeed" },
    ).speed;
    expect(speed).toMatchObject({
      signal: "VehicleSpeed",
      value: 88,
      label: "km/h",
      min: 0,
      max: 180,
      assigned: true,
    });
  });

  it("expands a flat assigned range so the slot still has a scale", () => {
    const gear = readingsFrom([], [signal({ name: "Gear", min: 3, max: 3 })], { gear: "Gear" }).gear;
    expect(gear.min).toBe(3);
    expect(gear.max).toBe(4);
    expect(gear.value).toBeNull();
  });

  it("falls back to the first sample default present in the log", () => {
    const bars = readingsFrom(
      [reading("FuelLevel", 40)],
      [signal({ name: "FuelLevel", min: 0, max: 100 })],
      {},
    );
    expect(bars.bar3).toMatchObject({ signal: "FuelLevel", value: 40, label: "FUEL", assigned: false });

    const withSoc = readingsFrom(
      [],
      [signal({ name: "Soc" }), signal({ name: "FuelLevel" }), signal({ name: "VehicleSpeed" })],
      { speed: "Missing" },
    );
    expect(withSoc.bar3).toMatchObject({ signal: "Soc", label: "SOC", assigned: false });
    expect(withSoc.speed).toMatchObject({
      signal: "VehicleSpeed",
      value: null,
      label: "km/h",
      min: 0,
      max: 320,
      decimals: 1,
      assigned: false,
    });
  });
});

describe("slotAt", () => {
  it("hits a slot inside its box and misses the gaps and clipped boxes", () => {
    expect(slotAt(58, 34, 100, 80)).toBe("speed");
    expect(slotAt(158, 34, 200, 80)).toBe("rpm");
    expect(slotAt(100, 30, 200, 80)).toBe("gear");
    expect(slotAt(220, 20, 300, 80)).toBe("bar1");
    expect(slotAt(370, 40, 500, 80)).toBe("lamp1");
    expect(slotAt(630, 40, 700, 80)).toBe("lamp6");
    expect(slotAt(90, 34, 200, 80)).toBeNull();
    expect(slotAt(58, 34, 80, 80)).toBeNull();
  });
});

describe("custom cluster widgets", () => {
  it("resolves custom labels, scales, lamp conditions, enum labels, and math values", () => {
    const widgets: ClusterWidget[] = [
      { id: "w1", kind: "value", signal: "BatteryVoltage", label: "BATT", decimals: 2, color: "#3ec6ff" },
      { id: "w2", kind: "lamp", signal: "CoolantTemp", label: "OVERHEAT", lampOp: ">=", lampThreshold: 105 },
      { id: "w3", kind: "digit", signal: "DriveMode", label: "MODE" },
      { id: "w4", kind: "arc", signal: "Slip", label: "SLIP", min: -10, max: 10, warnFrom: 5 },
    ];
    const signals: SignalInfo[] = [
      signal({ name: "BatteryVoltage", unit: "V", min: 11, max: 15 }),
      signal({ name: "CoolantTemp", unit: "°C", min: 40, max: 130 }),
      signal({
        name: "DriveMode",
        min: 0,
        max: 2,
        step: 1,
        labels: [
          { value: 0, label: "ECO" },
          { value: 1, label: "SPORT" },
          { value: 2, label: "TRACK" },
        ],
      }),
      signal({ name: "Slip", unit: "km/h", messageName: "Math", min: null, max: null, fromMap: false }),
    ];
    const resolved = resolveWidgets(
      widgets,
      [reading("BatteryVoltage", 14.25), reading("CoolantTemp", 108), reading("DriveMode", 2)],
      signals,
      new Map([["Slip", 6.5]]),
    );

    expect(resolved[0]).toMatchObject({ label: "BATT", value: 14.25, decimals: 2, unit: "V" });
    expect(resolved[1]).toMatchObject({ label: "OVERHEAT", lit: true });
    expect(resolved[2]).toMatchObject({ label: "MODE", enumLabel: "TRACK" });
    expect(resolved[3]).toMatchObject({ label: "SLIP", value: 6.5, min: -10, max: 10, warnFrom: 5 });
  });

  it("evaluates lamp operators and chooses smart widget kinds", () => {
    expect(evalLamp(10, ">", 10)).toBe(false);
    expect(evalLamp(10, ">=", 10)).toBe(true);
    expect(evalLamp(5, "<", 10)).toBe(true);
    expect(evalLamp(3, "==", 3)).toBe(true);
    expect(evalLamp(3, "!=", 3)).toBe(false);
    expect(evalLamp(null, ">", 0)).toBe(false);

    expect(smartWidgetForSignal(signal({ name: "AbsActive", min: 0, max: 1, step: 1 }), "a").kind).toBe("lamp");
    expect(
      smartWidgetForSignal(signal({ name: "Gear", min: 0, max: 6, step: 1, labels: [{ value: 0, label: "N" }] }), "b")
        .kind,
    ).toBe("digit");
    expect(smartWidgetForSignal(signal({ name: "Throttle", unit: "%", min: 0, max: 100 }), "c").kind).toBe("bar");
    expect(smartWidgetForSignal(signal({ name: "EngineRPM", unit: "rpm", min: 0, max: 8000 }), "d").kind).toBe("arc");
    expect(smartWidgetForSignal(signal({ name: "BatteryVoltage", unit: "V", min: 10, max: 15 }), "e").kind).toBe(
      "value",
    );
  });

  it("lays out widgets dynamically and supports hit-testing and drop indices", () => {
    const defaults = defaultClusterWidgets();
    const resolved = resolveWidgets(defaults, [], []);
    const { items, totalWidth } = layoutWidgets(resolved, 72);
    expect(items).toHaveLength(defaults.length);
    expect(totalWidth).toBeGreaterThan(500);
    expect(widgetAt(items[0].cx, items[0].cy, 900, 72, resolved)).toBe(0);
    expect(widgetAt(items[2].cx, items[2].cy, 900, 72, resolved)).toBe(2);
    expect(dropIndexAt(0, resolved, 72)).toBe(0);
    expect(dropIndexAt(totalWidth + 50, resolved, 72)).toBe(resolved.length);
    expect(serializeClusterWidgets([{ id: "x", kind: "lamp", label: "  HOT  ", lampThreshold: 50 }])).toEqual([
      { id: "x", kind: "lamp", label: "HOT", lampThreshold: 50 },
    ]);
  });
});
