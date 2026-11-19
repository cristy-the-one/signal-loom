import { describe, expect, it } from "vitest";
import {
  baseName,
  compareName,
  defaultPlotted,
  duration,
  hasData,
  isCompareName,
  oneSecond,
  signalEntries,
  windowFor,
} from "./model";
import type { SignalInfo, Summary } from "./types";

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

function summary(partial: Partial<Summary> = {}): Summary {
  return {
    logLabel: "log",
    logPath: null,
    mapLabel: null,
    mapPath: null,
    format: "slog",
    frameCount: 0,
    eventCount: 0,
    eventsTruncated: false,
    checkpointCount: 0,
    tStartUs: 0,
    tEndUs: 10_000_000,
    bytes: 0,
    signals: [],
    events: [],
    skippedRecords: 0,
    warnings: [],
    timeoutFactor: 2.5,
    mapMatch: null,
    ...partial,
  };
}

describe("duration", () => {
  it("is at least one microsecond", () => {
    expect(duration(summary({ tStartUs: 0, tEndUs: 100 }))).toBe(100);
    expect(duration(summary({ tStartUs: 5, tEndUs: 5 }))).toBe(1);
    expect(duration(summary({ tStartUs: 10, tEndUs: 3 }))).toBe(1);
  });
});

describe("hasData", () => {
  it("accepts a decoded sample or a math channel", () => {
    expect(hasData(signal({ name: "Speed", min: 0 }))).toBe(true);
    expect(hasData(signal({ name: "Diff", min: null, max: null, messageName: "Math" }))).toBe(true);
    expect(hasData(signal({ name: "Empty", min: null, max: null, messageName: "ECM" }))).toBe(false);
    expect(hasData(signal({ name: "Empty", min: null, max: null, messageName: "math" }))).toBe(true);
  });
});

describe("defaultPlotted", () => {
  it("returns the preferred names that have data, in preferred order", () => {
    const plotted = defaultPlotted(
      summary({
        signals: [
          signal({ name: "BrakePressure", min: 0, max: 5 }),
          signal({ name: "OilTemp", min: 70, max: 90 }),
          signal({ name: "EngineRPM", min: null, max: null, messageName: "ECM" }),
          signal({ name: "VehicleSpeed", min: 0, max: 10 }),
        ],
      }),
    );
    expect(plotted).toEqual(["VehicleSpeed", "BrakePressure"]);
  });

  it("skips checksums and counters, then prefers signals that move, capped at three", () => {
    expect(
      defaultPlotted(
        summary({
          signals: [
            signal({ name: "CRC_A", min: 0, max: 2 }),
            signal({ name: "Coolant", min: 1, max: 2 }),
            signal({ name: "AliveCounter", min: 0, max: 3 }),
          ],
        }),
      ),
    ).toEqual(["Coolant"]);

    expect(
      defaultPlotted(
        summary({
          signals: [
            signal({ name: "Flat", min: 1, max: 1 }),
            signal({ name: "Moves", min: 0, max: 2 }),
            signal({ name: "Also", min: 0, max: 3 }),
            signal({ name: "More", min: 1, max: 4 }),
            signal({ name: "Extra", min: 0, max: 5 }),
          ],
        }),
      ),
    ).toEqual(["Moves", "Also", "More"]);
  });

  it("falls back to housekeeping, a math channel, or nothing", () => {
    expect(
      defaultPlotted(
        summary({
          signals: [signal({ name: "Checksum", min: 0, max: 1 }), signal({ name: "Counter", min: 0, max: 2 })],
        }),
      ),
    ).toEqual(["Checksum", "Counter"]);
    expect(
      defaultPlotted(
        summary({
          signals: [signal({ name: "WheelDiff", min: null, max: null, messageName: "Math" })],
        }),
      ),
    ).toEqual(["WheelDiff"]);
    expect(defaultPlotted(summary())).toEqual([]);
  });
});

describe("signalEntries", () => {
  it("keeps the first signal for a repeated name and does not leak the previous summary", () => {
    const first = signal({ name: "Speed", messageName: "IC" });
    const repeat = signal({ name: "Speed", messageName: "ECM" });
    const entries = signalEntries(summary({ signals: [first, repeat, signal({ name: "Rpm" })] }));
    expect(entries.get("Speed")?.signal).toBe(first);
    expect(entries.get("Speed")?.index).toBe(0);
    expect(entries.get("Rpm")?.index).toBe(2);

    const next = signalEntries(summary({ signals: [signal({ name: "Brake" })] }));
    expect([...next.keys()]).toEqual(["Brake"]);
  });
});

describe("compare trace names", () => {
  it("adds and removes the compare suffix", () => {
    expect(compareName("VehicleSpeed")).toBe("VehicleSpeed · B");
    expect(baseName("VehicleSpeed · B")).toBe("VehicleSpeed");
    expect(baseName("VehicleSpeed")).toBe("VehicleSpeed");
    expect(baseName("VehicleSpeed·B")).toBe("VehicleSpeed·B");
    expect(isCompareName("VehicleSpeed · B")).toBe(true);
    expect(isCompareName("VehicleSpeed")).toBe(false);
    expect(isCompareName(" · B")).toBe(true);
    expect(baseName(" · B")).toBe("");
  });
});

describe("windowFor", () => {
  const log = summary({ tStartUs: 0, tEndUs: 10_000_000 });

  it("centers the span on the playhead and pins it inside the log", () => {
    expect(windowFor(5_000_000, 2_000_000, log)).toEqual({ t0: 4_000_000, t1: 6_000_000 });
    expect(windowFor(100_000, 2_000_000, log)).toEqual({ t0: 0, t1: 2_000_000 });
    expect(windowFor(9_900_000, 2_000_000, log)).toEqual({ t0: 8_000_000, t1: 10_000_000 });
  });

  it("shows the whole log when the span covers it, and enforces the minimum span", () => {
    expect(windowFor(5_000_000, 50_000_000, log)).toEqual({ t0: 0, t1: 10_000_000 });
    expect(windowFor(5_000_000, 100, log)).toEqual({ t0: 4_995_000, t1: 5_005_000 });
  });
});

describe("oneSecond", () => {
  it("returns one second around the playhead, clipped to the log", () => {
    const log = summary({ tStartUs: 0, tEndUs: 10_000_000 });
    expect(oneSecond(5_000_000, log)).toEqual({ t0: 4_500_000, t1: 5_500_000 });
    expect(oneSecond(100_000, log)).toEqual({ t0: 0, t1: 1_000_000 });
    expect(oneSecond(9_900_000, log)).toEqual({ t0: 9_000_000, t1: 10_000_000 });
    expect(oneSecond(100_000, summary({ tStartUs: 0, tEndUs: 200_000 }))).toEqual({ t0: 0, t1: 200_000 });
  });
});
