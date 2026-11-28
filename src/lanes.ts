import type { ConcreteTraceStyle, SignalInfo, TraceStyle } from "./types";

export type LaneMode = "overlay" | "split" | "grouped";

export const LANE_MODES: readonly LaneMode[] = ["overlay", "split", "grouped"];

export const TRACE_STYLES: readonly TraceStyle[] = ["auto", "step", "line", "area", "state", "flag", "heat"];

export const TRACE_STYLE_META: Record<TraceStyle, { label: string; short: string; desc: string }> = {
  auto: { label: "Auto", short: "Auto", desc: "Detect best view from signal metadata" },
  step: { label: "Step", short: "Step", desc: "CAN sample-and-hold staircase" },
  line: { label: "Line", short: "Line", desc: "Linear continuous waveform" },
  area: { label: "Area", short: "Area", desc: "Filled area waveform" },
  state: { label: "State bar", short: "State", desc: "Color-coded solid enum / state ribbon" },
  flag: { label: "Logic flag", short: "Flag", desc: "Active / inactive logic pulse band" },
  heat: { label: "Heat strip", short: "Heat", desc: "Continuous min-to-max color strip" },
};

const CHECKSUM_RE = /checksum|crc|\bchk\b/i;
const HOUSEKEEPING_RE = /checksum|crc|\bchk\b|counter|alive/i;

export function isChecksumSignal(name: string): boolean {
  return CHECKSUM_RE.test(name);
}

export function isHousekeepingSignal(name: string): boolean {
  return HOUSEKEEPING_RE.test(name);
}

/** Pick the concrete visualization style for a signal, honoring any manual override. */
export function resolveTraceStyle(
  signal: SignalInfo | undefined,
  override?: TraceStyle,
): ConcreteTraceStyle {
  if (override && override !== "auto") return override;
  if (signal?.labels && signal.labels.length > 0) return "state";
  if (signal && !signal.unit.trim() && signal.step === 1 && signal.min != null && signal.max != null) {
    if (signal.min >= 0 && signal.max <= 1) return "flag";
    if (
      signal.min >= 0 &&
      signal.max - signal.min <= 12 &&
      Number.isInteger(signal.min) &&
      Number.isInteger(signal.max) &&
      !HOUSEKEEPING_RE.test(signal.name)
    ) {
      return "state";
    }
  }
  return "step";
}

export function isRibbonStyle(style: ConcreteTraceStyle): boolean {
  return style === "state" || style === "flag" || style === "heat";
}

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
  /** True when every signal in this track renders as a compact ribbon (`state`, `flag`, `heat`). */
  compact?: boolean;
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

function autoGroupKey(name: string, info: SignalInfo | undefined, ribbon: boolean): string {
  if (ribbon) return `ribbon:${name}`;
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
  const real = signals.filter((s) => !isChecksumSignal(s));
  const pool = real.length > 0 ? real : signals;
  if (pool.length <= 1) {
    const u = lookup(pool[0] ?? "")?.unit?.trim() ?? "";
    return { shared: true, unit: u };
  }
  const units = pool.map((name) => lookup(name)?.unit?.trim() ?? "");
  const first = units[0];
  const sameUnit = units.every((u) => u.toLowerCase() === first.toLowerCase());
  return { shared: sameUnit, unit: sameUnit ? first : "" };
}

/**
 * Partition `visible` base signal names into horizontal scope tracks according to `mode`:
 * - `overlay`: analog signals share one track; ribbon signals (`state`/`flag`/`heat`) each get a compact lane beneath
 * - `split`: one track per signal (checksum signals fold into their parent message's track if present rather than taking a separate lane)
 * - `grouped`: manual `customLanes[name]` (`1..N`) overrides, otherwise auto-grouped by unit / message (ribbons get compact lanes; checksums never spawn a standalone lane when other signals are plotted)
 */
export function buildLaneSpecs(
  visible: readonly string[],
  mode: LaneMode,
  customLanes: Readonly<Record<string, number>>,
  lookup: (name: string) => SignalInfo | undefined,
  signalStyles?: Readonly<Record<string, TraceStyle>>,
): SignalLaneSpec[] {
  if (visible.length === 0) return [];

  const styleOf = (name: string) => resolveTraceStyle(lookup(name), signalStyles?.[name]);

  if (mode === "overlay") {
    const analog: string[] = [];
    const ribbons: string[] = [];
    for (const name of visible) {
      if (isRibbonStyle(styleOf(name))) ribbons.push(name);
      else analog.push(name);
    }
    const specs: SignalLaneSpec[] = [];
    if (analog.length > 0) {
      const { unit } = canShareScale(analog, lookup);
      specs.push({
        id: "overlay",
        badge: "T1",
        title: analog.length === 1 ? (unit ? `${analog[0]} (${unit})` : analog[0]) : "Overlay",
        unit,
        signals: analog,
        sharedScale: analog.length === 1,
        compact: false,
      });
    }
    for (const name of ribbons) {
      const unit = lookup(name)?.unit?.trim() ?? "";
      specs.push({
        id: `ribbon:${name}`,
        badge: `T${specs.length + 1}`,
        title: unit ? `${name} (${unit})` : name,
        unit,
        signals: [name],
        sharedScale: true,
        compact: true,
      });
    }
    return specs;
  }

  if (mode === "split") {
    const nonChecksum = visible.filter((n) => !isChecksumSignal(n));
    const checksums = visible.filter((n) => isChecksumSignal(n));
    const primary = nonChecksum.length > 0 ? nonChecksum : visible;
    const specs: SignalLaneSpec[] = primary.map((name, idx) => {
      const unit = lookup(name)?.unit?.trim() ?? "";
      const compact = isRibbonStyle(styleOf(name));
      return {
        id: `split:${name}`,
        badge: `T${idx + 1}`,
        title: unit ? `${name} (${unit})` : name,
        unit,
        signals: [name],
        sharedScale: true,
        compact,
      };
    });
    if (nonChecksum.length > 0) {
      for (const chk of checksums) {
        const msg = lookup(chk)?.messageName;
        const target =
          (msg ? specs.find((s) => lookup(s.signals[0])?.messageName === msg && !s.compact) : undefined) ??
          specs.find((s) => !s.compact) ??
          specs[specs.length - 1];
        if (target) target.signals.push(chk);
      }
    }
    return specs;
  }

  const buckets = new Map<string, string[]>();
  const deferredChecksums: string[] = [];
  for (const name of visible) {
    const manual = customLanes[name];
    if (isChecksumSignal(name) && !(manual && manual > 0)) {
      deferredChecksums.push(name);
      continue;
    }
    const ribbon = isRibbonStyle(styleOf(name));
    const key = manual && manual > 0 ? `manual:${manual}` : autoGroupKey(name, lookup(name), ribbon);
    const list = buckets.get(key);
    if (list) list.push(name);
    else buckets.set(key, [name]);
  }

  // Fold checksums into their parent message's lane (or an existing analog lane) instead of spawning a dedicated checksum lane.
  for (const chk of deferredChecksums) {
    if (buckets.size === 0) {
      const key = autoGroupKey(chk, lookup(chk), false);
      buckets.set(key, [chk]);
      continue;
    }
    const msg = lookup(chk)?.messageName;
    let targetKey: string | null = null;
    if (msg) {
      for (const [key, list] of buckets) {
        if (!key.startsWith("ribbon:") && list.some((s) => lookup(s)?.messageName === msg)) {
          targetKey = key;
          break;
        }
      }
    }
    if (!targetKey) {
      for (const key of buckets.keys()) {
        if (!key.startsWith("ribbon:")) targetKey = key;
      }
    }
    targetKey ??= [...buckets.keys()][buckets.size - 1];
    buckets.get(targetKey)!.push(chk);
  }

  let index = 0;
  const specs: SignalLaneSpec[] = [];
  for (const [key, signals] of buckets) {
    index += 1;
    const badge = `T${index}`;
    const realSignals = signals.filter((s) => !isChecksumSignal(s));
    const displaySignals = realSignals.length > 0 ? realSignals : signals;
    const compact = displaySignals.every((sig) => isRibbonStyle(styleOf(sig)));
    const { shared, unit } = canShareScale(signals, lookup);
    let title: string;
    if (displaySignals.length === 1) {
      title = unit ? `${displaySignals[0]} (${unit})` : displaySignals[0];
    } else if (unit) {
      title = `${badge} · ${unit} (${displaySignals.length})`;
    } else if (key.startsWith("msg:")) {
      title = `${badge} · ${key.slice(4)} (${displaySignals.length})`;
    } else {
      title = `${badge} · ${displaySignals.join(", ")}`;
    }
    specs.push({
      id: key,
      badge,
      title,
      unit,
      signals,
      sharedScale: compact ? true : shared,
      compact,
    });
  }
  return specs;
}

/** Sort `specs` according to user-arranged `order` of lane ids, preserving natural order for unseen lanes. */
export function orderLaneSpecs(
  specs: readonly SignalLaneSpec[],
  order: readonly string[],
): SignalLaneSpec[] {
  if (specs.length <= 1 || order.length === 0) return [...specs];
  const rank = new Map<string, number>();
  order.forEach((id, i) => rank.set(id, i));
  const indexed = specs.map((spec, natural) => ({
    spec,
    natural,
    pos: rank.get(spec.id) ?? Number.POSITIVE_INFINITY,
  }));
  indexed.sort((a, b) => a.pos - b.pos || a.natural - b.natural);
  return indexed.map(({ spec }, idx) => {
    const badge = `T${idx + 1}`;
    const title = spec.title.replace(/^T\d+\s·/, `${badge} ·`);
    return { ...spec, badge, title };
  });
}

/** Return a new ordered list of lane ids after moving `fromIndex` to insertion slot `insertIndex`. */
export function reorderLaneIds(
  ids: readonly string[],
  fromIndex: number,
  insertIndex: number,
): string[] {
  if (fromIndex < 0 || fromIndex >= ids.length) return [...ids];
  const next = [...ids];
  const [moved] = next.splice(fromIndex, 1);
  const target = insertIndex > fromIndex ? insertIndex - 1 : insertIndex;
  next.splice(Math.max(0, Math.min(next.length, target)), 0, moved);
  return next;
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
