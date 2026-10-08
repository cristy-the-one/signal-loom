import * as api from "./api";
import { els } from "./dom";
import { clamp, errText } from "./format";
import { MIN_SPAN, duration, markers, windowFor } from "./model";
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

export function scrubTo(t: number): void {
  if (!state.summary) return;
  state.playhead = clamp(t, state.summary.tStartUs, state.summary.tEndUs);
  state.dirty = true;
  requestDraw();
  scheduleRefresh();
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
    state.frame = frame;
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
  scrubTo(hit.t);
}

export function zoom(factor: number, anchor: number | null = null): void {
  if (!state.summary) return;
  const summary = state.summary;
  const old = windowFor(state.playhead, state.span, summary);
  const nextSpan = clamp(state.span * factor, MIN_SPAN, duration(summary));
  if (anchor != null && nextSpan < duration(summary) - 0.5) {
    const u = (anchor - old.t0) / Math.max(1, old.t1 - old.t0);
    state.playhead = clamp(anchor + nextSpan * (0.5 - u), summary.tStartUs, summary.tEndUs);
  }
  state.span = nextSpan;
  state.dirty = true;
  requestDraw();
  scheduleRefresh();
}

export function togglePlay(): void {
  if (!state.summary) return;
  state.playing = !state.playing;
  if (state.playing && state.playhead >= state.summary.tEndUs) state.playhead = state.summary.tStartUs;
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
    scrubTo(state.playhead + (event.deltaY / 400) * state.span);
    return;
  }
  const factor = Math.exp(event.deltaY * 0.0012);
  const target = event.currentTarget;
  let anchor: number | null = null;
  if (target === els.plot) {
    anchor = timeOnPlot(els.plot, event.clientX, windowFor(state.playhead, state.span, state.summary));
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
  els.rate.addEventListener("click", () => {
    const index = RATES.indexOf(state.rate);
    state.rate = RATES[(index + 1) % RATES.length];
    renderTransport();
  });
  els.zoomIn.addEventListener("click", () => zoom(0.7));
  els.zoomOut.addEventListener("click", () => zoom(1.4));
  els.zoomAll.addEventListener("click", () => {
    if (!state.summary) return;
    state.span = duration(state.summary);
    state.dirty = true;
    draw();
    scheduleRefresh();
  });

  let panX = 0;
  let panning = false;
  els.plot.addEventListener("pointermove", (event) => {
    if (!state.summary) return;
    if (panning) {
      const rect = els.plot.getBoundingClientRect();
      const dx = event.clientX - panX;
      panX = event.clientX;
      const view = windowFor(state.playhead, state.span, state.summary);
      scrubTo(state.playhead + (-dx / Math.max(1, rect.width)) * (view.t1 - view.t0));
      return;
    }
    const view = windowFor(state.playhead, state.span, state.summary);
    state.hoverT = timeOnPlot(els.plot, event.clientX, view);
    state.hoverX = event.clientX;
    requestDraw();
  });
  els.plot.addEventListener("pointerleave", () => {
    state.hoverT = null;
    requestDraw();
  });

  const scrubTimeline = (event: PointerEvent) => {
    if (!state.summary) return;
    const t = timeAt(els.timeline, event.clientX, {
      t0: state.summary.tStartUs,
      t1: state.summary.tEndUs,
    });
    scrubTo(t);
  };
  els.timeline.addEventListener("pointerdown", (event) => {
    els.timeline.setPointerCapture(event.pointerId);
    scrubTimeline(event);
  });
  els.timeline.addEventListener("pointermove", (event) => {
    if (event.buttons & 1) scrubTimeline(event);
  });
  els.plot.addEventListener("pointerdown", (event) => {
    if (!state.summary) return;
    if (event.button === 1 || event.altKey) {
      panning = true;
      panX = event.clientX;
      els.plot.setPointerCapture(event.pointerId);
      return;
    }
    if (event.button !== 0) return;
    scrubTo(timeOnPlot(els.plot, event.clientX, windowFor(state.playhead, state.span, state.summary)));
  });
  els.plot.addEventListener("pointerup", () => {
    panning = false;
  });
  els.plot.addEventListener("wheel", onWheel, { passive: false });
  els.timeline.addEventListener("wheel", onWheel, { passive: false });
}
