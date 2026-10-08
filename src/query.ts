import * as api from "./api";
import { els } from "./dom";
import { errText, formatSpan, formatUs, formatValue } from "./format";
import { oneSecond, windowFor } from "./model";
import { renderTransport } from "./readout";
import { bumpDataGen, dataGen, draw, state } from "./state";
import { clearError, setError } from "./status";
import type { Series } from "./types";

let queryFlight = false;
let queryAgain = false;
let refreshTimer = 0;
let cursorSeq = 0;

/** Changes the plotted set; plot responses and cursor stats asked for the old set are dropped. */
export function setPlotted(names: string[]): void {
  state.plotted = names;
  bumpDataGen();
  void refreshCursors();
}

export function scheduleRefresh(): void {
  window.clearTimeout(refreshTimer);
  refreshTimer = window.setTimeout(() => void refresh(), 40);
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
  const view = windowFor(state.playhead, state.span, summary);
  const width = els.plot.getBoundingClientRect().width || 800;
  const maxPoints = Math.max(200, Math.min(4000, Math.round(width * 2)));
  try {
    const bus = oneSecond(state.playhead, summary);
    const [series, frame, held, load] = await Promise.all([
      state.plotted.length
        ? api.query({
            t0Us: view.t0,
            t1Us: view.t1,
            signals: state.plotted,
            maxPoints,
            includeCompare: state.compareOn,
          })
        : Promise.resolve([] as Series[]),
      api.frameAt(state.playhead),
      api.valuesAt(state.playhead),
      api.busLoad(bus.t0, bus.t1),
    ]);
    if (gen !== dataGen()) return;
    state.series = series;
    state.frame = frame;
    state.held = held;
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

export async function refreshOverview(): Promise<void> {
  const summary = state.summary;
  const name = state.plotted[0];
  if (!summary || !name) {
    state.overview = null;
    state.overviewName = null;
    draw();
    return;
  }
  if (state.overviewName === name && state.overview) return;
  const gen = dataGen();
  try {
    const series = await api.query({
      t0Us: summary.tStartUs,
      t1Us: summary.tEndUs,
      signals: [name],
      maxPoints: 700,
    });
    if (gen !== dataGen()) return;
    state.overview = series[0]?.points ?? null;
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
