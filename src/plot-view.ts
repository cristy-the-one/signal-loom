import * as api from "./api";
import { drawBus, drawWidgets, resolveWidgets } from "./gauges";
import { els } from "./dom";
import { severityOf } from "./family";
import { formatSpan, formatTimeForSpan, readoutFor } from "./format";
import { buildLaneSpecs, isChecksumSignal, orderLaneSpecs, resolveTraceStyle } from "./lanes";
import { parseMath } from "./math-eval";
import { activeView, baseName, colorFor, compareName, isCompareName, labelForValue, signalNamed } from "./model";
import { clusterInteraction } from "./panels/cluster";
import { syncEventSelection } from "./panels/events";
import { renderLegend } from "./panels/legend";
import {
  computeLaneLoupeWindows,
  drawLaneLoupe,
  drawPlot,
  formatHover,
  heldValue,
  type PlotLane,
  type Trace,
} from "./plot";
import { mathRange } from "./query";
import { renderTransport } from "./readout";
import { dataGen, draw, requestDraw, state } from "./state";
import { drawTimeline, type TimeRange, type TimelineMark } from "./timeline";
import type { Point, Summary } from "./types";

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

let lastLanes: PlotLane[] = [];
let laneHoverPill = -1;
let laneDragIndex = -1;
let laneDropIndex: number | null = null;
let loupeState: { range: TimeRange; factor: number; anchorX: number } | null = null;
let laneLoupeDock: { side: "left" | "right"; vside: "top" | "bottom" } | null = null;
let laneLoupeFetchSeq = 0;
let laneLoupeInFlightKey = "";
let laneLoupeCache: {
  gen: number;
  sigKey: string;
  t0: number;
  t1: number;
  byName: Map<string, Point[]>;
} | null = null;

function loupeSigKey(): string {
  return `${state.compareOn ? 1 : 0}|${state.plotted.join("\u0001")}`;
}

function cacheCovers(t0: number, t1: number, margin = 0): boolean {
  return Boolean(
    laneLoupeCache &&
      laneLoupeCache.gen === dataGen() &&
      laneLoupeCache.sigKey === loupeSigKey() &&
      laneLoupeCache.t0 <= t0 - margin &&
      laneLoupeCache.t1 >= t1 + margin,
  );
}

/**
 * Fetch high-resolution samples for all plotted signals covering Level 1's window (±100% padding),
 * so the magnifier opens directly on crisp final data (zero tentative coarse frame) and `↑`/`↓`
 * lane switches stay instant from cache.
 */
export async function ensureLaneLoupeData(
  liveView: { t0: number; t1: number },
  focusT: number,
): Promise<void> {
  const summary = state.summary;
  if (!summary || state.plotted.length === 0) return;
  const viewSpan = Math.max(1, liveView.t1 - liveView.t0);
  if (viewSpan <= 10_000) return;

  const domain = { t0: summary.tStartUs, t1: summary.tEndUs };
  const [lvl1] = computeLaneLoupeWindows(liveView, focusT, domain);
  const span = Math.max(1, lvl1.t1 - lvl1.t0);
  // Keep a 25% safety margin so gentle sliding prefetches the next tile before running off cache.
  if (cacheCovers(lvl1.t0, lvl1.t1, span * 0.25)) return;

  const q0 = Math.max(summary.tStartUs, Math.floor(lvl1.t0 - span));
  const q1 = Math.min(summary.tEndUs, Math.ceil(lvl1.t1 + span));
  const gen = dataGen();
  const sigKey = loupeSigKey();
  const flightKey = `${gen}|${sigKey}|${q0}|${q1}`;
  if (flightKey === laneLoupeInFlightKey) return;

  const seq = ++laneLoupeFetchSeq;
  laneLoupeInFlightKey = flightKey;
  try {
    const series = await api.query({
      t0Us: q0,
      t1Us: q1,
      signals: [...state.plotted],
      maxPoints: 2400,
      includeCompare: state.compareOn,
    });
    if (gen !== dataGen() || seq !== laneLoupeFetchSeq) return;
    laneLoupeCache = {
      gen,
      sigKey,
      t0: q0,
      t1: q1,
      byName: new Map(series.map((s) => [s.name, s.points])),
    };
  } catch {
    // Fall back cleanly to the already-loaded primary series.
  } finally {
    if (laneLoupeInFlightKey === flightKey) laneLoupeInFlightKey = "";
  }
}

export function cancelLaneLoupeFetch(): void {
  laneLoupeFetchSeq += 1;
  laneLoupeInFlightKey = "";
}

/** Current micro-map loupe range & anchor, managed by `transport.ts` on timeline hover. */
export function currentLoupe(): { range: TimeRange; factor: number; anchorX: number } | null {
  return loupeState;
}

export function setLoupeState(next: { range: TimeRange; factor: number; anchorX: number } | null): void {
  loupeState = next;
}

/** Latest rendered lanes, used by `transport.ts` for pill click-to-focus & drag-to-reorder. */
export function currentPlotLanes(): PlotLane[] {
  return lastLanes;
}

export function setLaneDragOverlay(next: {
  hoverPillIndex?: number;
  dragLaneIndex?: number;
  dropLaneIndex?: number | null;
}): void {
  if (next.hoverPillIndex !== undefined) laneHoverPill = next.hoverPillIndex;
  if (next.dragLaneIndex !== undefined) laneDragIndex = next.dragLaneIndex;
  if (next.dropLaneIndex !== undefined) laneDropIndex = next.dropLaneIndex;
}

/** Build tracks and traces according to `state.laneMode` (`overlay` / `split` / `grouped`). */
function buildLanes(): { traces: Trace[]; lanes: PlotLane[]; hasMixedScale: boolean } {
  const byName = new Map(state.series.map((item) => [item.name, item]));
  const visibleBases = [...state.plotted];
  const rawSpecs = buildLaneSpecs(visibleBases, state.laneMode, state.signalLanes, signalNamed, state.signalStyles);
  const specs = orderLaneSpecs(rawSpecs, state.laneOrder);
  const allTraces: Trace[] = [];
  let hasMixedScale = false;

  const focusSignal = state.solo && state.plotted.includes(state.solo) ? state.solo : null;
  const focusLaneId =
    state.focusedLane && specs.some((s) => s.id === state.focusedLane) ? state.focusedLane : null;
  const hasFocus = focusSignal != null || focusLaneId != null;

  const lanes: PlotLane[] = specs.map((spec) => {
    const laneIsFocused =
      focusLaneId != null
        ? spec.id === focusLaneId
        : focusSignal != null
          ? spec.signals.includes(focusSignal)
          : false;
    const laneIsDimmed = hasFocus && !laneIsFocused;

    const laneTraces: Trace[] = [];
    const hasRealSignals = spec.signals.some((s) => !isChecksumSignal(s));
    for (const sig of spec.signals) {
      const trace = makeRawTrace(sig, byName);
      if (hasRealSignals && isChecksumSignal(sig)) trace.muted = true;
      if (focusSignal != null) {
        if (sig === focusSignal) trace.focused = true;
        else if (laneIsFocused) trace.dimmed = true;
      }
      laneTraces.push(trace);
      if (state.compareOn && byName.has(compareName(sig))) {
        const cmp = makeRawTrace(compareName(sig), byName);
        if (hasRealSignals && isChecksumSignal(sig)) cmp.muted = true;
        if (focusSignal != null && sig !== focusSignal && laneIsFocused) cmp.dimmed = true;
        laneTraces.push(cmp);
      }
    }
    const scaleTraces = laneTraces.filter((t) => !t.muted);
    const activeForScale = scaleTraces.length > 0 ? scaleTraces : laneTraces;
    let scaleMin = 0;
    let scaleMax = 1;
    if (spec.sharedScale && activeForScale.length > 0) {
      scaleMin = Math.min(...activeForScale.map((t) => t.min));
      scaleMax = Math.max(...activeForScale.map((t) => t.max));
      if (!Number.isFinite(scaleMin) || !Number.isFinite(scaleMax)) {
        scaleMin = 0;
        scaleMax = 1;
      }
      for (const t of activeForScale) {
        t.min = scaleMin;
        t.max = scaleMax;
      }
    } else if (!spec.compact && activeForScale.length > 1) {
      hasMixedScale = true;
    }
    allTraces.push(...laneTraces);
    return {
      id: spec.id,
      title: spec.title,
      traces: laneTraces,
      sharedScale: spec.sharedScale,
      scaleMin,
      scaleMax,
      compact: spec.compact,
      focused: laneIsFocused,
      dimmed: laneIsDimmed,
    };
  });

  lastLanes = lanes;
  return { traces: allTraces, lanes, hasMixedScale };
}

function placeCrosshair(
  traces: Trace[],
  liveView: { t0: number; t1: number },
): void {
  const viewSpan = Math.max(1, liveView.t1 - liveView.t0);
  if (state.hoverT == null || !state.summary || traces.length === 0) {
    els.crosshair.hidden = true;
    return;
  }
  els.crosshair.hidden = false;
  const stage = els.plot.parentElement?.getBoundingClientRect();
  const plotRect = els.plot.getBoundingClientRect();
  if (!stage) return;

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

  // Position away from the mouse pointer in both X and Y so the cursor never covers the readout.
  const boxW = els.crosshair.offsetWidth || 190;
  const boxH = els.crosshair.offsetHeight || 60;
  const cursorX = state.hoverX - stage.left;
  const cursorY = state.hoverY - stage.top;
  const gapX = 22;
  const gapY = 20;

  let left = cursorX + gapX;
  if (left + boxW > stage.width - 8) {
    left = Math.max(8, cursorX - boxW - gapX);
  }
  const minTop = Math.max(6, plotRect.top - stage.top + 6);
  const maxTop = Math.max(minTop, stage.height - boxH - 8);
  let top = cursorY + gapY;
  if (top > maxTop) {
    top = Math.max(minTop, cursorY - boxH - gapY);
  }
  els.crosshair.style.left = `${Math.round(left)}px`;
  els.crosshair.style.top = `${Math.round(top)}px`;
}

/** Separate, lazy-displayed 2-level lane magnifier docked on the opposite side of the stage from the cursor. */
function placeLaneLoupe(
  lanes: PlotLane[],
  liveView: { t0: number; t1: number },
): void {
  const hoveredLane =
    state.laneLoupe &&
    state.laneLoupeOpen &&
    state.hoverT != null &&
    state.summary &&
    state.hoverLane >= 0 &&
    state.hoverLane < lanes.length
      ? lanes[state.hoverLane]
      : null;

  if (!hoveredLane || hoveredLane.traces.length === 0 || !state.summary || state.hoverT == null) {
    cancelLaneLoupeFetch();
    laneLoupeDock = null;
    els.laneLoupePanel.hidden = true;
    return;
  }

  const stage = els.plot.parentElement?.getBoundingClientRect();
  const plotRect = els.plot.getBoundingClientRect();
  if (!stage) return;

  els.laneLoupePanel.hidden = false;
  const viewSpan = Math.max(1, liveView.t1 - liveView.t0);
  const domain = { t0: state.summary.tStartUs, t1: state.summary.tEndUs };
  const [lvl1, lvl2] = computeLaneLoupeWindows(liveView, state.hoverT, domain);

  // Proactively top up the padded high-res tile before the pointer slides past its margin.
  if (viewSpan > 10_000 && !cacheCovers(lvl1.t0, lvl1.t1, (lvl1.t1 - lvl1.t0) * 0.25)) {
    void ensureLaneLoupeData(liveView, state.hoverT).then(() => {
      if (state.hoverT != null && state.laneLoupe && state.laneLoupeOpen) requestDraw();
    });
  }
  const hiRes = cacheCovers(lvl1.t0, lvl1.t1) ? laneLoupeCache!.byName : null;

  const loupeLane: PlotLane = hiRes
    ? {
        ...hoveredLane,
        traces: hoveredLane.traces.map((t) => {
          const pts = hiRes.get(t.name);
          return pts && pts.length > 0 ? { ...t, points: pts, endT: laneLoupeCache!.t1 } : t;
        }),
      }
    : hoveredLane;

  // Choose a dock corner once when the magnifier opens and keep it rock-steady;
  // only relocate if the mouse pointer actually enters the magnifier's own rectangle.
  const cursorX = state.hoverX - stage.left;
  const cursorY = state.hoverY - stage.top;
  const plotTop = Math.max(8, Math.round(plotRect.top - stage.top + 10));
  const plotBottom = Math.max(plotTop + 40, Math.round(plotRect.bottom - stage.top - 26));
  const panelW = els.laneLoupePanel.offsetWidth || Math.min(404, stage.width - 28);
  const panelH = els.laneLoupePanel.offsetHeight || 226;

  if (!laneLoupeDock) {
    laneLoupeDock = {
      side: cursorX < stage.width * 0.5 ? "right" : "left",
      vside: cursorY < (plotTop + plotBottom) * 0.5 ? "bottom" : "top",
    };
  } else {
    const pad = 16;
    const pLeft = laneLoupeDock.side === "left" ? 12 : stage.width - 12 - panelW;
    const pRight = pLeft + panelW;
    const pTop = laneLoupeDock.vside === "top" ? plotTop : Math.max(plotTop, plotBottom - panelH);
    const pBottom = pTop + panelH;
    const collides =
      cursorX >= pLeft - pad &&
      cursorX <= pRight + pad &&
      cursorY >= pTop - pad &&
      cursorY <= pBottom + pad;
    if (collides) {
      laneLoupeDock = {
        side: laneLoupeDock.side === "right" ? "left" : "right",
        vside: laneLoupeDock.vside,
      };
    }
  }

  const top =
    laneLoupeDock.vside === "top"
      ? plotTop
      : Math.max(plotTop, Math.round(plotBottom - panelH));
  els.laneLoupePanel.style.top = `${top}px`;
  if (laneLoupeDock.side === "right") {
    els.laneLoupePanel.style.left = "auto";
    els.laneLoupePanel.style.right = "12px";
  } else {
    els.laneLoupePanel.style.right = "auto";
    els.laneLoupePanel.style.left = "12px";
  }

  els.laneLoupeTitle.textContent = hoveredLane.title || "Overlay";
  els.laneLoupe1Tag.textContent = `L1 · ${lvl1.factor}× · ${formatSpan(lvl1.t1 - lvl1.t0)}`;
  els.laneLoupe2Tag.textContent = `L2 · ${lvl2.factor}× · ${formatSpan(lvl2.t1 - lvl2.t0)}`;
  drawLaneLoupe(els.laneLoupe1, loupeLane, lvl1, state.hoverT, state.playhead, {
    innerBracket: lvl2,
  });
  drawLaneLoupe(els.laneLoupe2, loupeLane, lvl2, state.hoverT, state.playhead, {
    showSampleDots: true,
  });
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
      hoverPillIndex: laneHoverPill,
      dragLaneIndex: laneDragIndex,
      dropLaneIndex: laneDropIndex,
      loupeLaneIndex: state.laneLoupe && state.laneLoupeOpen ? state.hoverLane : -1,
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
  drawTimeline(els.timeline, domain, liveView, state.playhead, marks, state.overview, loupeState?.range ?? null);
  if (summary && loupeState) {
    els.timelineLoupe.hidden = false;
    const wrapRect = els.timelineWrap.getBoundingClientRect();
    const loupeW = Math.min(420, Math.max(260, wrapRect.width - 24));
    els.timelineLoupe.style.width = `${loupeW}px`;
    const localX = loupeState.anchorX - wrapRect.left;
    const left = Math.max(8, Math.min(localX - loupeW / 2, wrapRect.width - loupeW - 8));
    els.timelineLoupe.style.left = `${Math.round(left)}px`;
    const span = Math.max(1, loupeState.range.t1 - loupeState.range.t0);
    els.timelineLoupeTitle.textContent = `MICRO-MAP · ${loupeState.factor}× · ${formatSpan(span)}`;
    els.timelineLoupeRange.textContent = `${formatTimeForSpan(loupeState.range.t0, span)} – ${formatTimeForSpan(loupeState.range.t1, span)}`;
    drawTimeline(els.timelineLoupeCanvas, loupeState.range, liveView, state.playhead, marks, state.overview);
  } else {
    els.timelineLoupe.hidden = true;
  }
  placeCrosshair(traces, liveView);
  placeLaneLoupe(lanes, liveView);
  renderTransport();
  syncEventSelection();
}

export function bindPlotView(): void {
  window.addEventListener("resize", () => draw());
  new ResizeObserver(() => draw()).observe(els.viewport);
}
