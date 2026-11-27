import * as api from "./api";
import { els } from "./dom";
import { needsFetch, planFetch, type Fetched } from "./fetch-window";
import { errText, formatSpan, formatUs, formatValue } from "./format";
import { activeView, oneSecond } from "./model";
import { renderTransport } from "./readout";
import { bumpDataGen, dataGen, draw, state } from "./state";
import { clearError, setError } from "./status";
import type { Point, Series, Summary } from "./types";

let queryFlight = false;
let queryAgain = false;
let refreshTimer = 0;
let cursorSeq = 0;
let lastSeriesKey = "";
let fetched: Fetched | null = null;
let mathRangeSummary: Summary | null = null;
const mathRanges = new Map<string, { min: number; max: number } | "pending">();
let overviewCacheGen = -1;
const overviewCache = new Map<string, Point[]>();
const OVERVIEW_CACHE_CAP = 8;

/** Changes the plotted set; plot responses and cursor stats asked for the old set are dropped. */
export function setPlotted(names: string[]): void {
  state.plotted = names;
  if (state.solo && !names.includes(state.solo)) state.solo = null;
  bumpDataGen();
  void refreshCursors();
}

export function scheduleRefresh(): void {
  window.clearTimeout(refreshTimer);
  refreshTimer = window.setTimeout(() => void refresh(), 32);
}

export async function refresh(): Promise<void> {
  const summary = state.summary;
  if (!summary) {
    draw();
    return;
  }
  if (queryFlight) {
    queryAgain = true;
    return;
  }
  queryFlight = true;
  const gen = dataGen();
  const view = activeView(summary);
  const width = els.plot.getBoundingClientRect().width || 800;
  const bounds = { t0: summary.tStartUs, t1: summary.tEndUs };
  const seriesKey = state.plotted.length
    ? `${gen}|${state.compareOn ? 1 : 0}|${state.plotted.join("")}`
    : "";
  const reuseSeries =
    seriesKey !== "" && seriesKey === lastSeriesKey && !needsFetch(fetched, view, bounds, width);
  const plan = seriesKey === "" ? null : reuseSeries ? fetched : planFetch(view, bounds, width);
  void refreshMathRanges(summary);
  try {
    const bus = oneSecond(state.playhead, summary);
    const [series, frame, load] = await Promise.all([
      !plan
        ? Promise.resolve([] as Series[])
        : reuseSeries
          ? Promise.resolve(state.series)
          : api.query({
              t0Us: plan.t0,
              t1Us: plan.t1,
              signals: state.plotted,
              maxPoints: plan.maxPoints,
              includeCompare: state.compareOn,
            }),
      api.frameAt(state.playhead),
      api.busLoad(bus.t0, bus.t1),
    ]);
    if (gen !== dataGen()) return;
    lastSeriesKey = seriesKey;
    fetched = plan;
    state.series = series;
    state.seriesRange = plan ? { t0: plan.t0, t1: plan.t1 } : null;
    state.frame = frame;
    state.held = frame?.values ?? [];
    state.bus = load;
    state.view = view;
    clearError("query");
    draw();
  } catch (err) {
    if (gen === dataGen()) setError(errText(err), "query");
  } finally {
    queryFlight = false;
    if (queryAgain) {
      queryAgain = false;
      void refresh();
    }
  }
}

/** A math channel's range over the whole log, once fetched; logged signals carry theirs in the summary. */
export function mathRange(name: string): { min: number; max: number } | null {
  if (mathRangeSummary !== state.summary) return null;
  const range = mathRanges.get(mathKey(name));
  return range && range !== "pending" ? range : null;
}

function mathKey(name: string): string {
  const channel = state.math.find((item) => item.name === name);
  return channel ? `${channel.name}${channel.expr}` : name;
}

/**
 * Fetches the whole-log range of each plotted math channel that has none yet,
 * so its trace keeps one scale while panning. Bucketed min/max points keep a
 * series' extremes, so a coarse whole-log query is enough.
 */
async function refreshMathRanges(summary: Summary): Promise<void> {
  if (mathRangeSummary !== summary) {
    mathRanges.clear();
    mathRangeSummary = summary;
  }
  const names = state.plotted.filter(
    (name) => state.math.some((channel) => channel.name === name) && !mathRanges.has(mathKey(name)),
  );
  if (names.length === 0) return;
  const keys = names.map(mathKey);
  for (const key of keys) mathRanges.set(key, "pending");
  try {
    const series = await api.query({
      t0Us: summary.tStartUs,
      t1Us: summary.tEndUs,
      signals: names,
      maxPoints: 2_000,
    });
    if (mathRangeSummary !== summary) return;
    names.forEach((name, i) => {
      const points = series.find((item) => item.name === name)?.points ?? [];
      let min = Infinity;
      let max = -Infinity;
      for (const point of points) {
        if (point.v < min) min = point.v;
        if (point.v > max) max = point.v;
      }
      if (Number.isFinite(min) && max > min) mathRanges.set(keys[i], { min, max });
      else mathRanges.delete(keys[i]);
    });
    draw();
  } catch {
    if (mathRangeSummary === summary) for (const key of keys) mathRanges.delete(key);
  }
}

export async function refreshOverview(): Promise<void> {
  const summary = state.summary;
  const name = state.plotted[0];
  if (!summary || !name) {
    state.overview = null;
    state.overviewName = null;
    draw();
    return;
  }
  const gen = dataGen();
  if (overviewCacheGen !== gen) {
    overviewCache.clear();
    overviewCacheGen = gen;
  }
  const cached = overviewCache.get(name);
  if (cached) {
    state.overview = cached;
    state.overviewName = name;
    draw();
    return;
  }
  if (state.overviewName === name && state.overview) return;
  try {
    const series = await api.query({
      t0Us: summary.tStartUs,
      t1Us: summary.tEndUs,
      signals: [name],
      maxPoints: 700,
    });
    if (gen !== dataGen()) return;
    const pts = series[0]?.points ?? null;
    if (pts) {
      if (overviewCache.size >= OVERVIEW_CACHE_CAP) {
        const oldest = overviewCache.keys().next().value;
        if (oldest !== undefined) overviewCache.delete(oldest);
      }
      overviewCache.set(name, pts);
    }
    state.overview = pts;
    state.overviewName = name;
    draw();
  } catch {
    if (gen === dataGen()) state.overview = null;
  }
}

/** Drops any cursor stats still in flight. */
export function cancelCursorStats(): void {
  cursorSeq += 1;
}

export async function refreshCursors(): Promise<void> {
  const seq = ++cursorSeq;
  const gen = dataGen();
  const a = state.cursorA;
  const b = state.cursorB;
  if (a == null && b == null) {
    state.cursorText = "";
    renderTransport();
    return;
  }
  if (a == null || b == null || state.plotted.length === 0) {
    const mark = a ?? b ?? 0;
    state.cursorText = `${a == null ? "B" : "A"} ${formatUs(mark)}`;
    renderTransport();
    draw();
    return;
  }
  const t0 = Math.min(a, b);
  const t1 = Math.max(a, b);
  const name = state.plotted[0];
  let text: string;
  try {
    const stats = await api.signalStats(name, t0, t1);
    text = `Δt ${formatSpan(t1 - t0)} · ${name} Δ ${formatValue(stats.last - stats.first)} · min ${formatValue(stats.min)} max ${formatValue(stats.max)} avg ${formatValue(stats.avg)}`;
  } catch (err) {
    text = errText(err);
  }
  if (seq !== cursorSeq || gen !== dataGen()) return;
  state.cursorText = text;
  renderTransport();
  draw();
}
