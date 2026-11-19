import { describe, expect, it } from "vitest";
import { readingsFrom, slotAt, slotSpec } from "./gauges";
import type { SignalInfo, ValueRead } from "./types";

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
