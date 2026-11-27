import * as api from "./api";
import { els } from "./dom";
import { clamp, errText } from "./format";
import { nextLaneMode } from "./lanes";
import { MIN_SPAN, activeView, duration, markers, windowFor } from "./model";
import { refresh, scheduleRefresh } from "./query";
import { renderTransport } from "./readout";
import { draw, requestDraw, state } from "./state";
import { clearError, setError, setNotice } from "./status";
import { timeOnPlot } from "./plot";
import { timeAt } from "./timeline";

const RATES = [1, 4, 16];

let lastFetch = 0;
let playStamp = 0;
let tickFrame = 0;

/** Move playhead to `t`. When `followCamera` is true (or `t` falls outside the visible window), recenter camera. */
export function scrubTo(t: number, followCamera = true): void {
  if (!state.summary) return;
  const summary = state.summary;
  const next = clamp(t, summary.tStartUs, summary.tEndUs);
  state.playhead = next;
  if (followCamera) {
    state.viewCenter = null;
  } else {
    const view = activeView(summary);
    if (next < view.t0 || next > view.t1) {
      state.viewCenter = next;
    }
  }
  state.dirty = true;
  requestDraw();
  scheduleRefresh();
}

export function panBy(deltaUs: number): void {
  if (!state.summary) return;
  const summary = state.summary;
  const view = activeView(summary);
  const center = (view.t0 + view.t1) / 2;
  state.viewCenter = clamp(center + deltaUs, summary.tStartUs, summary.tEndUs);
  state.dirty = true;
  requestDraw();
  scheduleRefresh();
}

export function resetZoom(): void {
  if (!state.summary) return;
  state.span = duration(state.summary);
  state.viewCenter = null;
  state.boxZoom = null;
  state.dirty = true;
  draw();
  scheduleRefresh();
}

export function cycleLaneMode(): void {
  state.laneMode = nextLaneMode(state.laneMode);
  if (state.laneMode === "split") {
    setNotice("Scope: Split tracks (1 track per signal).");
  } else if (state.laneMode === "grouped") {
    setNotice("Scope: Grouped tracks — click T1/T2 badges in the legend to customize groups.");
  } else {
    setNotice("Scope: Single overlay track.");
  }
  draw();
}

export async function stepFrame(direction: "next" | "prev"): Promise<void> {
  if (!state.summary) return;
  try {
    const frame = await api.step(state.playhead, direction);
    if (!frame) {
      setNotice(direction === "next" ? "End of log." : "Start of log.");
      return;
    }
    clearError("action");
    state.playhead = frame.tUs;
    const view = activeView(state.summary);
    if (frame.tUs < view.t0 || frame.tUs > view.t1) {
      state.viewCenter = frame.tUs;
    }
    state.frame = frame;
    state.held = frame.values;
    state.dirty = true;
    requestDraw();
    scheduleRefresh();
  } catch (err) {
    setError(errText(err), "action");
  }
}

export function stepEvent(direction: 1 | -1): void {
  const list = markers();
  if (list.length === 0) {
    setNotice("No events or bookmarks in this log.");
    return;
  }
  const hit =
    direction > 0
      ? list.find((item) => item.t > state.playhead + 0.5)
      : [...list].reverse().find((item) => item.t < state.playhead - 0.5);
  if (!hit) {
    setNotice(direction > 0 ? "No later event." : "No earlier event.");
    return;
  }
  scrubTo(hit.t, true);
}

export function zoom(factor: number, anchor: number | null = null): void {
  if (!state.summary) return;
  const summary = state.summary;
  const old = activeView(summary);
  const nextSpan = clamp(state.span * factor, MIN_SPAN, duration(summary));
  if (nextSpan >= duration(summary) - 0.5) {
    state.viewCenter = null;
  } else if (anchor != null) {
    const u = (anchor - old.t0) / Math.max(1, old.t1 - old.t0);
    const nextCenter = anchor + nextSpan * (0.5 - u);
    const fitted = windowFor(nextCenter, nextSpan, summary);
    state.viewCenter = (fitted.t0 + fitted.t1) / 2;
  } else if (state.viewCenter == null) {
    state.viewCenter = state.playhead;
  }
  state.span = nextSpan;
  state.dirty = true;
  requestDraw();
  scheduleRefresh();
}

export function togglePlay(): void {
  if (!state.summary) return;
  state.playing = !state.playing;
  if (state.playing) {
    state.viewCenter = null;
    if (state.playhead >= state.summary.tEndUs) state.playhead = state.summary.tStartUs;
  }
  renderTransport();
  if (state.playing) {
    playStamp = 0;
    tickFrame = requestAnimationFrame(tick);
  } else {
    cancelAnimationFrame(tickFrame);
    tickFrame = 0;
  }
}

function tick(now: number): void {
  tickFrame = 0;
  if (!state.playing || !state.summary) return;
  if (!playStamp) playStamp = now;
  const dt = now - playStamp;
  playStamp = now;
  state.playhead += dt * 1000 * state.rate;
  if (state.playhead >= state.summary.tEndUs) {
    state.playhead = state.summary.tEndUs;
    state.playing = false;
  }
  draw();
  if (now - lastFetch > 90) {
    lastFetch = now;
    void refresh();
  }
  if (state.playing) tickFrame = requestAnimationFrame(tick);
}

function onWheel(event: WheelEvent): void {
  if (!state.summary) return;
  event.preventDefault();
  if (event.shiftKey) {
    panBy((event.deltaY / 400) * state.span);
    return;
  }
  const factor = Math.exp(event.deltaY * 0.0012);
  const target = event.currentTarget;
  let anchor: number | null = null;
  if (target === els.plot) {
    anchor = timeOnPlot(els.plot, event.clientX, activeView(state.summary));
  } else if (target === els.timeline) {
    anchor = timeAt(els.timeline, event.clientX, { t0: state.summary.tStartUs, t1: state.summary.tEndUs });
  }
  zoom(factor, anchor);
}

export function bindTransport(): void {
  els.prevEvent.addEventListener("click", () => stepEvent(-1));
  els.nextEvent.addEventListener("click", () => stepEvent(1));
  els.prevFrame.addEventListener("click", () => void stepFrame("prev"));
  els.nextFrame.addEventListener("click", () => void stepFrame("next"));
  els.play.addEventListener("click", togglePlay);
  els.laneMode.addEventListener("click", cycleLaneMode);
  els.rate.addEventListener("click", () => {
    const index = RATES.indexOf(state.rate);
    state.rate = RATES[(index + 1) % RATES.length];
    renderTransport();
  });
  els.zoomIn.addEventListener("click", () => zoom(0.7));
  els.zoomOut.addEventListener("click", () => zoom(1.4));
  els.zoomAll.addEventListener("click", resetZoom);

  let panX = 0;
  let mode: "idle" | "pan" | "scrub" | "box" = "idle";
  let boxStartT = 0;
  let boxStartX = 0;

  els.plot.addEventListener("pointermove", (event) => {
    if (!state.summary) return;
    const view = activeView(state.summary);
    if (mode === "pan") {
      const rect = els.plot.getBoundingClientRect();
      const dx = event.clientX - panX;
      panX = event.clientX;
      panBy((-dx / Math.max(1, rect.width)) * (view.t1 - view.t0));
      return;
    }
    if (mode === "box") {
      const curT = timeOnPlot(els.plot, event.clientX, view);
      state.boxZoom = { t0: boxStartT, t1: curT };
      state.hoverT = curT;
      state.hoverX = event.clientX;
      requestDraw();
      return;
    }
    if (mode === "scrub") {
      const t = timeOnPlot(els.plot, event.clientX, view);
      state.hoverT = t;
      state.hoverX = event.clientX;
      scrubTo(t, false);
      return;
    }
    state.hoverT = timeOnPlot(els.plot, event.clientX, view);
    state.hoverX = event.clientX;
    requestDraw();
  });

  els.plot.addEventListener("pointerleave", () => {
    if (mode === "idle") {
      state.hoverT = null;
      requestDraw();
    }
  });

  els.plot.addEventListener("pointerdown", (event) => {
    if (!state.summary) return;
    const view = activeView(state.summary);
    if (event.button === 1 || event.altKey) {
      mode = "pan";
      panX = event.clientX;
      els.plot.setPointerCapture(event.pointerId);
      return;
    }
    if (event.button !== 0) return;
    if (event.shiftKey) {
      mode = "box";
      boxStartX = event.clientX;
      boxStartT = timeOnPlot(els.plot, event.clientX, view);
      state.boxZoom = { t0: boxStartT, t1: boxStartT };
      els.plot.setPointerCapture(event.pointerId);
      requestDraw();
      return;
    }
    // Pin the current camera center so clicking near an edge inspects that spike without jumping the stage.
    state.viewCenter = (view.t0 + view.t1) / 2;
    mode = "scrub";
    els.plot.setPointerCapture(event.pointerId);
    scrubTo(timeOnPlot(els.plot, event.clientX, view), false);
  });

  els.plot.addEventListener("pointerup", (event) => {
    if (mode === "box" && state.summary && state.boxZoom) {
      const dragPx = Math.abs(event.clientX - boxStartX);
      const lo = Math.min(state.boxZoom.t0, state.boxZoom.t1);
      const hi = Math.max(state.boxZoom.t0, state.boxZoom.t1);
      state.boxZoom = null;
      mode = "idle";
      if (dragPx >= 6 && hi > lo) {
        const nextSpan = clamp(hi - lo, MIN_SPAN, duration(state.summary));
        const fitted = windowFor((lo + hi) / 2, nextSpan, state.summary);
        state.span = nextSpan;
        state.viewCenter = (fitted.t0 + fitted.t1) / 2;
        state.dirty = true;
        draw();
        scheduleRefresh();
      } else {
        requestDraw();
      }
      return;
    }
    mode = "idle";
  });

  els.plot.addEventListener("pointercancel", () => {
    state.boxZoom = null;
    mode = "idle";
    requestDraw();
  });

  els.plot.addEventListener("dblclick", () => {
    resetZoom();
  });

  let timelineDraggingWindow = false;
  let timelineLastT = 0;

  els.timeline.addEventListener("pointerdown", (event) => {
    if (!state.summary) return;
    els.timeline.setPointerCapture(event.pointerId);
    const domain = { t0: state.summary.tStartUs, t1: state.summary.tEndUs };
    const t = timeAt(els.timeline, event.clientX, domain);
    const view = activeView(state.summary);
    const zoomedIn = view.t1 - view.t0 < duration(state.summary) * 0.98;
    if (zoomedIn && t >= view.t0 && t <= view.t1) {
      timelineDraggingWindow = true;
      timelineLastT = t;
    } else {
      timelineDraggingWindow = false;
      scrubTo(t, true);
    }
  });

  els.timeline.addEventListener("pointermove", (event) => {
    if (!(event.buttons & 1) || !state.summary) return;
    const domain = { t0: state.summary.tStartUs, t1: state.summary.tEndUs };
    const t = timeAt(els.timeline, event.clientX, domain);
    if (timelineDraggingWindow) {
      const dt = t - timelineLastT;
      timelineLastT = t;
      panBy(dt);
    } else {
      scrubTo(t, true);
    }
  });

  els.timeline.addEventListener("pointerup", () => {
    timelineDraggingWindow = false;
  });

  els.plot.addEventListener("wheel", onWheel, { passive: false });
  els.timeline.addEventListener("wheel", onWheel, { passive: false });
}
