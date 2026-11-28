import { drawBus, drawWidgets, resolveWidgets } from "./gauges";
import { els } from "./dom";
import { severityOf } from "./family";
import { formatTimeForSpan, readoutFor } from "./format";
import { buildLaneSpecs, resolveTraceStyle } from "./lanes";
import { parseMath } from "./math-eval";
import { activeView, baseName, colorFor, compareName, isCompareName, labelForValue, signalNamed } from "./model";
import { clusterInteraction } from "./panels/cluster";
import { syncEventSelection } from "./panels/events";
import { renderLegend } from "./panels/legend";
import { drawPlot, formatHover, heldValue, type PlotLane, type Trace } from "./plot";
import { mathRange } from "./query";
import { renderTransport } from "./readout";
import { draw, state } from "./state";
import { drawTimeline, type TimelineMark } from "./timeline";
import type { Summary } from "./types";

let eventMarksOf: Summary | null = null;
let eventMarksCache: TimelineMark[] = [];

function eventMarksFor(summary: Summary | null): TimelineMark[] {
  if (!summary) return [];
  if (eventMarksOf === summary) return eventMarksCache;
  eventMarksCache = summary.events.map((event) => ({
    t: event.tUs,
    kind: "event" as const,
    severity: severityOf(event.label),
  }));
  eventMarksOf = summary;
  return eventMarksCache;
}

function stageMessage(): string | null {
  if (!state.summary) return "No log on the deck. Load the synthetic hypercar sample or open a CAN log.";
  if (state.summary.signals.length === 0) return "No signals yet. Open a JSON signal map to decode frames.";
  if (state.plotted.length === 0) return "Select signals to overlay them on the scope.";
  return null;
}

/** Live values for defined math channels at the current playhead (for cluster gadgets & legend). */
export function liveMathValues(): Map<string, number> {
  const out = new Map<string, number>();
  if (state.math.length === 0) return out;
  const bySeries = new Map(state.series.map((s) => [s.name, s.points]));
  const heldMap = new Map(state.held.map((h) => [h.name, h.value]));
  for (const ch of state.math) {
    const pts = bySeries.get(ch.name);
    if (pts && pts.length > 0) {
      const v = heldValue(pts, state.playhead);
      if (v != null) {
        out.set(ch.name, v);
        continue;
      }
    }
    const parsed = parseMath(ch.expr);
    if (parsed.ok) {
      const v = parsed.evaluate(heldMap);
      if (v != null) out.set(ch.name, v);
    }
  }
  return out;
}

function makeRawTrace(name: string, byName: Map<string, { points: { t: number; v: number }[] }>): Trace {
  const base = baseName(name);
  const series = byName.get(name);
  const points = series?.points ?? [];
  const info = signalNamed(base);
  const whole = info?.min == null ? mathRange(base) : null;
  let min = info?.min ?? whole?.min ?? null;
  let max = info?.max ?? whole?.max ?? null;
  if (min == null || max == null || max <= min) {
    const scalePoints = points.length ? points : (byName.get(base)?.points ?? []);
    min = scalePoints.reduce((lo, point) => Math.min(lo, point.v), Infinity);
    max = scalePoints.reduce((hi, point) => Math.max(hi, point.v), -Infinity);
    if (!Number.isFinite(min) || !Number.isFinite(max)) {
      min = 0;
      max = 1;
    }
  }
  return {
    name,
    color: colorFor(name),
    points,
    min,
    max,
    dashed: isCompareName(name),
    endT: state.seriesRange?.t1,
    style: resolveTraceStyle(info, state.signalStyles[base]),
    labels: info?.labels,
  };
}

/** Build tracks and traces according to `state.laneMode` (`overlay` / `split` / `grouped`). */
function buildLanes(): { traces: Trace[]; lanes: PlotLane[]; hasMixedScale: boolean } {
  const byName = new Map(state.series.map((item) => [item.name, item]));
  const visibleBases =
    state.solo && state.plotted.includes(state.solo) ? [state.solo] : [...state.plotted];
  const specs = buildLaneSpecs(visibleBases, state.laneMode, state.signalLanes, signalNamed, state.signalStyles);
  const allTraces: Trace[] = [];
  let hasMixedScale = false;

  const lanes: PlotLane[] = specs.map((spec) => {
    const laneTraces: Trace[] = [];
    for (const sig of spec.signals) {
      laneTraces.push(makeRawTrace(sig, byName));
      if (state.compareOn && byName.has(compareName(sig))) {
        laneTraces.push(makeRawTrace(compareName(sig), byName));
      }
    }
    let scaleMin = 0;
    let scaleMax = 1;
    if (spec.sharedScale && laneTraces.length > 0) {
      scaleMin = Math.min(...laneTraces.map((t) => t.min));
      scaleMax = Math.max(...laneTraces.map((t) => t.max));
      if (!Number.isFinite(scaleMin) || !Number.isFinite(scaleMax)) {
        scaleMin = 0;
        scaleMax = 1;
      }
      for (const t of laneTraces) {
        t.min = scaleMin;
        t.max = scaleMax;
      }
    } else if (!spec.compact && laneTraces.length > 1) {
      hasMixedScale = true;
    }
    allTraces.push(...laneTraces);
    return {
      title: spec.title,
      traces: laneTraces,
      sharedScale: spec.sharedScale,
      scaleMin,
      scaleMax,
      compact: spec.compact,
    };
  });

  return { traces: allTraces, lanes, hasMixedScale };
}

function placeCrosshair(traces: Trace[], viewSpan: number): void {
  if (state.hoverT == null || !state.summary || traces.length === 0) {
    els.crosshair.hidden = true;
    return;
  }
  els.crosshair.hidden = false;
  const stage = els.plot.parentElement?.getBoundingClientRect();
  const plotRect = els.plot.getBoundingClientRect();
  if (!stage) return;
  let left = state.hoverX - stage.left + 14;
  if (left > stage.width - 220) left = Math.max(8, state.hoverX - stage.left - 220);
  els.crosshair.style.left = `${left}px`;
  els.crosshair.style.top = `${Math.max(8, plotRect.top - stage.top + 8)}px`;
  els.crossTime.textContent = formatTimeForSpan(state.hoverT, viewSpan);
  const nameWidth = Math.max(0, ...traces.map((trace) => trace.name.length));
  const lines = traces.map((trace) => {
    const signal = signalNamed(baseName(trace.name));
    const value = heldValue(trace.points, state.hoverT ?? 0);
    return formatHover(
      trace.name.padEnd(nameWidth),
      value,
      signal?.unit ?? "",
      readoutFor(signal),
      labelForValue(signal, value),
    );
  });
  els.crossVals.textContent = lines.join("\n");
}

/** Paints the stage, gauges, bus strip, timeline, crosshair, transport readouts and legend from the state. */
export function paint(): void {
  const summary = state.summary;
  const message = stageMessage();
  els.stageMsg.hidden = message == null;
  els.stageMsg.textContent = message ?? "";
  const mathVals = liveMathValues();
  // Render the legend first so `#plot` measures its true height when the legend bar toggles visibility.
  renderLegend(mathVals);
  const { traces, lanes, hasMixedScale } = buildLanes();
  els.scaleNote.hidden = !hasMixedScale;
  const liveView = summary ? activeView(summary) : state.view;
  drawPlot(
    els.plot,
    liveView,
    state.playhead,
    traces,
    {
      hoverT: state.hoverT,
      cursorA: state.cursorA,
      cursorB: state.cursorB,
      boxZoom: state.boxZoom,
    },
    lanes,
  );
  const resolved = summary
    ? resolveWidgets(state.clusterWidgets, state.held, summary.signals, mathVals)
    : null;
  drawWidgets(els.gauges, resolved, clusterInteraction());
  drawBus(els.bus, state.bus);
  const domain = summary
    ? { t0: summary.tStartUs, t1: summary.tEndUs }
    : { t0: 0, t1: 1 };
  const marks: TimelineMark[] = [
    ...eventMarksFor(summary),
    ...state.bookmarks.map((mark) => ({ t: mark.tUs, kind: "mark" as const })),
    ...state.notes.map((note) => ({ t: note.tUs, kind: "mark" as const })),
  ];
  drawTimeline(els.timeline, domain, liveView, state.playhead, marks, state.overview);
  placeCrosshair(traces, Math.max(1, liveView.t1 - liveView.t0));
  renderTransport();
  syncEventSelection();
}

export function bindPlotView(): void {
  window.addEventListener("resize", () => draw());
  new ResizeObserver(() => draw()).observe(els.viewport);
}
