import { familyColor } from "./family";
import { clamp } from "./format";
import { state, type View } from "./state";
import type { SignalInfo, Summary } from "./types";

export const MIN_SPAN = 10_000;

/** Suffix of a trace drawn from the compare drive. */
const COMPARE_SUFFIX = " · B";

export function duration(summary: Summary): number {
  return Math.max(1, summary.tEndUs - summary.tStartUs);
}

/** A signal has data when the log decoded at least one sample of it. */
export function hasData(signal: SignalInfo): boolean {
  return signal.min != null || signal.messageName === "Math";
}

/** Three signals worth a first look: ones with data, not checksums or counters. */
export function defaultPlotted(summary: Summary): string[] {
  const live = summary.signals.filter(hasData);
  const names = new Set(live.map((signal) => signal.name));
  const preferred = ["VehicleSpeed", "EngineRPM", "BrakePressure"].filter((name) => names.has(name));
  if (preferred.length) return preferred;
  const housekeeping = /checksum|counter|crc|alive/i;
  const useful = live.filter((signal) => !housekeeping.test(signal.name));
  const pool = useful.length ? useful : live;
  // A signal that never changes draws a flat line: prefer ones that move.
  const moves = (signal: SignalInfo) => signal.min != null && signal.max != null && signal.max > signal.min;
  return [...pool.filter(moves), ...pool.filter((signal) => !moves(signal))]
    .slice(0, 3)
    .map((signal) => signal.name);
}

interface SignalEntry {
  signal: SignalInfo;
  index: number;
  color: string | null;
}

let entriesOf: Summary | null = null;
let entries = new Map<string, SignalEntry>();

/** Name to signal, list position and colour for one summary; built once per summary, colours on first use. */
export function signalEntries(summary: Summary): Map<string, SignalEntry> {
  if (entriesOf === summary) return entries;
  const next = new Map<string, SignalEntry>();
  summary.signals.forEach((signal, index) => {
    if (!next.has(signal.name)) next.set(signal.name, { signal, index, color: null });
  });
  entriesOf = summary;
  entries = next;
  return next;
}

export function signalNamed(name: string): SignalInfo | undefined {
  return state.summary ? signalEntries(state.summary).get(name)?.signal : undefined;
}

/** The signal a trace shows: a compare trace carries its drive-A signal's name plus a suffix. */
export function baseName(trace: string): string {
  return trace.endsWith(COMPARE_SUFFIX) ? trace.slice(0, -COMPARE_SUFFIX.length) : trace;
}

export function compareName(name: string): string {
  return `${name}${COMPARE_SUFFIX}`;
}

export function isCompareName(trace: string): boolean {
  return trace.endsWith(COMPARE_SUFFIX);
}

export function colorFor(name: string): string {
  const base = baseName(name);
  const entry = state.summary ? signalEntries(state.summary).get(base) : undefined;
  if (!entry) return familyColor(base, "", 0);
  entry.color ??= familyColor(base, entry.signal.messageName ?? "", entry.index);
  return entry.color;
}

/** The plotted names, each followed by its compare-drive trace when that drive has one. */
export function traceNames(): string[] {
  const names = [...state.plotted];
  if (!state.compareOn) return names;
  for (const name of state.plotted) {
    if (state.series.some((series) => series.name === compareName(name))) names.push(compareName(name));
  }
  return names;
}

export function windowFor(playhead: number, span: number, summary: Summary): View {
  const start = summary.tStartUs;
  const end = summary.tEndUs;
  const dur = duration(summary);
  const width = clamp(span, MIN_SPAN, dur);
  if (width >= dur - 0.5) return { t0: start, t1: end };
  let a = playhead - width / 2;
  let b = a + width;
  if (a < start) {
    a = start;
    b = start + width;
  }
  if (b > end) {
    b = end;
    a = end - width;
  }
  return { t0: a, t1: b };
}

export function oneSecond(playhead: number, summary: Summary): View {
  const half = 500_000;
  let t0 = playhead - half;
  let t1 = playhead + half;
  if (t0 < summary.tStartUs) {
    t0 = summary.tStartUs;
    t1 = Math.min(summary.tEndUs, t0 + 1_000_000);
  }
  if (t1 > summary.tEndUs) {
    t1 = summary.tEndUs;
    t0 = Math.max(summary.tStartUs, t1 - 1_000_000);
  }
  return { t0, t1 };
}

/** Events and bookmarks as times, in order. */
export function markers(): { t: number }[] {
  const events = state.summary?.events.map((event) => ({ t: event.tUs })) ?? [];
  const marks = state.bookmarks.map((mark) => ({ t: mark.tUs }));
  return [...events, ...marks].sort((a, b) => a.t - b.t);
}
