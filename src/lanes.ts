import type { SignalInfo } from "./types";

export type LaneMode = "overlay" | "split" | "grouped";

export const LANE_MODES: readonly LaneMode[] = ["overlay", "split", "grouped"];

export interface SignalLaneSpec {
  id: string;
  /** Short badge shown on legend chips in `grouped` mode (`T1`, `T2`, …). */
  badge: string;
  /** Title rendered inside the lane when multiple tracks are stacked. */
  title: string;
  unit: string;
  /** Base signal names assigned to this track, in plotted order. */
  signals: string[];
  /** True when all traces in this track share one physical `[min, max]` Y scale. */
  sharedScale: boolean;
}

export function nextLaneMode(current: LaneMode): LaneMode {
  const idx = LANE_MODES.indexOf(current);
  return LANE_MODES[(idx + 1) % LANE_MODES.length];
}

export function laneModeLabel(mode: LaneMode): string {
  if (mode === "split") return "Split";
  if (mode === "grouped") return "Group";
  return "Overlay";
}

function autoGroupKey(name: string, info: SignalInfo | undefined): string {
  const unit = info?.unit?.trim() ?? "";
  if (unit) return `unit:${unit.toLowerCase()}`;
  const msg = info?.messageName?.trim() ?? "";
  if (msg && msg.toLowerCase() !== "math") return `msg:${msg}`;
  return `sig:${name}`;
}

function canShareScale(signals: string[], lookup: (name: string) => SignalInfo | undefined): {
  shared: boolean;
  unit: string;
} {
  if (signals.length <= 1) {
    const u = lookup(signals[0] ?? "")?.unit?.trim() ?? "";
    return { shared: true, unit: u };
  }
  const units = signals.map((name) => lookup(name)?.unit?.trim() ?? "");
  const first = units[0];
  const sameUnit = units.every((u) => u.toLowerCase() === first.toLowerCase());
  return { shared: sameUnit, unit: sameUnit ? first : "" };
}

/**
 * Partition `visible` base signal names into horizontal scope tracks according to `mode`:
 * - `overlay`: all signals on a single track
 * - `split`: one track per signal
 * - `grouped`: manual `customLanes[name]` (`1..N`) overrides, otherwise auto-grouped by unit / message
 */
export function buildLaneSpecs(
  visible: readonly string[],
  mode: LaneMode,
  customLanes: Readonly<Record<string, number>>,
  lookup: (name: string) => SignalInfo | undefined,
): SignalLaneSpec[] {
  if (visible.length === 0) return [];

  if (mode === "overlay") {
    const { unit } = canShareScale([...visible], lookup);
    return [
      {
        id: "overlay",
        badge: "T1",
        title: visible.length === 1 ? visible[0] : "Overlay",
        unit,
        signals: [...visible],
        sharedScale: visible.length === 1,
      },
    ];
  }

  if (mode === "split") {
    return visible.map((name, idx) => {
      const unit = lookup(name)?.unit?.trim() ?? "";
      return {
        id: `split:${name}`,
        badge: `T${idx + 1}`,
        title: unit ? `${name} (${unit})` : name,
        unit,
        signals: [name],
        sharedScale: true,
      };
    });
  }

  const buckets = new Map<string, string[]>();
  for (const name of visible) {
    const manual = customLanes[name];
    const key = manual && manual > 0 ? `manual:${manual}` : autoGroupKey(name, lookup(name));
    const list = buckets.get(key);
    if (list) list.push(name);
    else buckets.set(key, [name]);
  }

  let index = 0;
  const specs: SignalLaneSpec[] = [];
  for (const [key, signals] of buckets) {
    index += 1;
    const badge = `T${index}`;
    const { shared, unit } = canShareScale(signals, lookup);
    let title: string;
    if (signals.length === 1) {
      title = unit ? `${signals[0]} (${unit})` : signals[0];
    } else if (unit) {
      title = `${badge} · ${unit} (${signals.length})`;
    } else if (key.startsWith("msg:")) {
      title = `${badge} · ${key.slice(4)} (${signals.length})`;
    } else {
      title = `${badge} · ${signals.join(", ")}`;
    }
    specs.push({
      id: key,
      badge,
      title,
      unit,
      signals,
      sharedScale: shared,
    });
  }
  return specs;
}

/**
 * Cycle a signal's manual group assignment (`1 -> 2 -> ... -> maxGroup + 1 -> auto (0)`).
 * Returns a new `Record<string, number>` with `0` entries omitted.
 */
export function cycleSignalLane(
  name: string,
  plotted: readonly string[],
  current: Readonly<Record<string, number>>,
): Record<string, number> {
  const maxManual = plotted.reduce((hi, sig) => Math.max(hi, current[sig] ?? 0), 0);
  const cap = Math.min(Math.max(2, plotted.length), maxManual + 1);
  const cur = current[name] ?? 0;
  const nextVal = cur >= cap ? 0 : cur + 1;
  const next: Record<string, number> = {};
  for (const [sig, lane] of Object.entries(current)) {
    if (sig !== name && lane > 0) next[sig] = lane;
  }
  if (nextVal > 0) next[name] = nextVal;
  return next;
}
