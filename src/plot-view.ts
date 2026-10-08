import { drawBus, drawGauges, readingsFrom } from "./gauges";
import { els } from "./dom";
import { severityOf } from "./family";
import { formatUs, readoutFor } from "./format";
import { baseName, colorFor, isCompareName, signalNamed, traceNames, windowFor } from "./model";
import { renderLegend } from "./panels/legend";
import { syncEventSelection } from "./panels/events";
import { drawPlot, formatHover, heldValue, type Trace } from "./plot";
import { renderTransport } from "./readout";
import { draw, state } from "./state";
import { drawTimeline, type TimelineMark } from "./timeline";

function stageMessage(): string | null {
  if (!state.summary) return "No log on the deck. Load the synthetic hypercar sample or open a CAN log.";
  if (state.summary.signals.length === 0) return "No signals yet. Open a JSON signal map to decode frames.";
  if (state.plotted.length === 0) return "Select signals to overlay them on the scope.";
  return null;
}

/** One trace per plotted name, scaled by its signal's range, or by the points it has when the range is flat. */
function buildTraces(): Trace[] {
  return traceNames().map((name) => {
    const base = baseName(name);
    const series = state.series.find((item) => item.name === name);
    const points = series?.points ?? [];
    const info = signalNamed(base);
    let min = info?.min ?? null;
    let max = info?.max ?? null;
    if (min == null || max == null || max <= min) {
      const scalePoints = points.length ? points : (state.series.find((item) => item.name === base)?.points ?? []);
      min = scalePoints.reduce((lo, point) => Math.min(lo, point.v), Infinity);
      max = scalePoints.reduce((hi, point) => Math.max(hi, point.v), -Infinity);
      if (!Number.isFinite(min) || !Number.isFinite(max)) {
        min = 0;
        max = 1;
      }
    }
    return { name, color: colorFor(name), points, min, max, dashed: isCompareName(name) };
  });
}

function placeCrosshair(traces: Trace[]): void {
  if (state.hoverT == null || !state.summary) {
    els.crosshair.hidden = true;
    return;
  }
  els.crosshair.hidden = false;
  const stage = els.plot.parentElement?.getBoundingClientRect();
  if (!stage) return;
  let left = state.hoverX - stage.left + 14;
  if (left > stage.width - 200) left = Math.max(8, state.hoverX - stage.left - 200);
  els.crosshair.style.left = `${left}px`;
  els.crosshair.style.top = "18px";
  els.crossTime.textContent = formatUs(state.hoverT);
  const nameWidth = Math.max(0, ...traces.map((trace) => trace.name.length));
  const lines = traces.map((trace) => {
    const signal = signalNamed(baseName(trace.name));
    const value = heldValue(trace.points, state.hoverT ?? 0);
    return formatHover(trace.name.padEnd(nameWidth), value, signal?.unit ?? "", readoutFor(signal));
  });
  els.crossVals.textContent = lines.join("\n");
}

/** Paints the stage, gauges, bus strip, timeline, crosshair, transport readouts and legend from the state. */
export function paint(): void {
  const summary = state.summary;
  const message = stageMessage();
  els.stageMsg.hidden = message == null;
  els.stageMsg.textContent = message ?? "";
  const traces = buildTraces();
  drawPlot(els.plot, state.view, state.playhead, traces, {
    hoverT: state.hoverT,
    cursorA: state.cursorA,
    cursorB: state.cursorB,
  });
  drawGauges(els.gauges, state.summary ? readingsFrom(state.held, state.summary.signals, state.cluster) : null);
  drawBus(els.bus, state.bus);
  const domain = summary
    ? { t0: summary.tStartUs, t1: summary.tEndUs }
    : { t0: 0, t1: 1 };
  const view = summary ? windowFor(state.playhead, state.span, summary) : state.view;
  const marks: TimelineMark[] = [
    ...(summary?.events.map((event) => ({
      t: event.tUs,
      kind: "event" as const,
      severity: severityOf(event.label),
    })) ?? []),
    ...state.bookmarks.map((mark) => ({ t: mark.tUs, kind: "mark" as const })),
    ...state.notes.map((note) => ({ t: note.tUs, kind: "mark" as const })),
  ];
  drawTimeline(els.timeline, domain, view, state.playhead, marks, state.overview);
  placeCrosshair(traces);
  renderTransport();
  renderLegend();
  syncEventSelection();
}

export function bindPlotView(): void {
  window.addEventListener("resize", () => draw());
  new ResizeObserver(() => draw()).observe(els.viewport);
}
