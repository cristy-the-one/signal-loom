import * as api from "./api";
import { els } from "./dom";
import { clamp, errText } from "./format";
import { nextLaneMode, reorderLaneIds, type LaneMode } from "./lanes";
import { MIN_SPAN, activeView, duration, markers, windowFor } from "./model";
import { laneAt, laneCenterClientY, laneDropIndexAt, lanePillAt, timeOnPlot } from "./plot";
import {
  cancelLaneLoupeFetch,
  currentLoupe,
  currentPlotLanes,
  ensureLaneLoupeData,
  setLaneDragOverlay,
  setLoupeState,
} from "./plot-view";
import { refresh, scheduleRefresh } from "./query";
import { renderDirty, renderTransport } from "./readout";
import { draw, requestDraw, state } from "./state";
import { clearError, setError, setNotice } from "./status";
import { computeLoupeDomain, shouldShowLoupe, timeAt } from "./timeline";

export const RATES = [0.1, 0.25, 0.5, 1, 2, 4, 8, 16] as const;
/** Hover dwell before the lazy lane magnifier opens, so fast cursor sweeps stay uncluttered. */
const LANE_LOUPE_DWELL_MS = 450;

let lastFetch = 0;
let playStamp = 0;
let tickFrame = 0;
let laneLoupeDwellTimer = 0;
let laneLoupeAnchorX = 0;
let laneLoupeAnchorY = 0;
let laneLoupeDismissed = false;
let laneLoupeDismissX = 0;
let laneLoupeDismissY = 0;
let laneKeyLocked = false;
let laneKeyLockX = 0;
let laneKeyLockY = 0;

function cancelLaneLoupeDwell(): void {
  window.clearTimeout(laneLoupeDwellTimer);
  laneLoupeDwellTimer = 0;
  cancelLaneLoupeFetch();
}

export function dismissLaneLoupe(): void {
  cancelLaneLoupeDwell();
  laneKeyLocked = false;
  if (!state.laneLoupeOpen) return;
  state.laneLoupeOpen = false;
  laneLoupeDismissed = true;
  laneLoupeDismissX = state.hoverX;
  laneLoupeDismissY = state.hoverY;
  requestDraw();
}

/**
 * Step the inspected / magnified lane up or down via ArrowUp / ArrowDown without moving `hoverT`,
 * so you can rapidly A/B compare adjacent lanes at the exact same timestamp while the mouse stays still.
 */
export function stepHoverLane(delta: -1 | 1): boolean {
  const lanes = currentPlotLanes();
  if (lanes.length <= 1 || !state.summary) return false;
  if (state.hoverT == null) return false;
  const current = state.hoverLane >= 0 && state.hoverLane < lanes.length ? state.hoverLane : 0;
  const next = clamp(current + delta, 0, lanes.length - 1);
  if (next === current && state.laneLoupeOpen) return true;

  cancelLaneLoupeDwell();
  laneLoupeDismissed = false;
  if (!laneKeyLocked) {
    laneKeyLockX = state.hoverX;
    laneKeyLockY = state.hoverY;
    laneKeyLocked = true;
  }
  state.hoverLane = next;
  const centerY = laneCenterClientY(els.plot, next, lanes);
  if (centerY != null) state.hoverY = centerY;
  if (state.laneLoupe) {
    const view = activeView(state.summary);
    const focusT = state.hoverT;
    if (state.laneLoupeOpen) {
      requestDraw();
    } else {
      void ensureLaneLoupeData(view, focusT).then(() => {
        if (state.laneLoupe && state.hoverT === focusT && state.hoverLane === next) {
          state.laneLoupeOpen = true;
          requestDraw();
        }
      });
    }
  } else {
    requestDraw();
  }
  return true;
}

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

export function zoomToCursors(): void {
  if (!state.summary || state.cursorA == null || state.cursorB == null) {
    setNotice("Place both Cursor A (1) and Cursor B (2) to zoom to selection.");
    return;
  }
  const lo = Math.min(state.cursorA, state.cursorB);
  const hi = Math.max(state.cursorA, state.cursorB);
  if (hi <= lo) return;
  const pad = Math.round((hi - lo) * 0.05);
  const nextSpan = clamp(hi - lo + pad * 2, MIN_SPAN, duration(state.summary));
  const fitted = windowFor((lo + hi) / 2, nextSpan, state.summary);
  state.span = nextSpan;
  state.viewCenter = (fitted.t0 + fitted.t1) / 2;
  state.dirty = true;
  draw();
  scheduleRefresh();
}

export function centerOnPlayhead(): void {
  if (!state.summary) return;
  state.viewCenter = null;
  draw();
  scheduleRefresh();
}

export function stepRate(delta: -1 | 1): void {
  let idx = RATES.indexOf(state.rate as (typeof RATES)[number]);
  if (idx < 0) idx = RATES.indexOf(1);
  const nextIdx = clamp(idx + delta, 0, RATES.length - 1);
  if (nextIdx === idx) {
    setNotice(delta > 0 ? `Already at max playback speed (${state.rate}×).` : `Already at slowest playback speed (${state.rate}×).`);
    return;
  }
  state.rate = RATES[nextIdx];
  renderTransport();
}

export function resetRate(): void {
  state.rate = 1;
  renderTransport();
}

export function cycleRate(): void {
  const index = RATES.indexOf(state.rate as (typeof RATES)[number]);
  state.rate = RATES[(index + 1) % RATES.length];
  renderTransport();
}

export function moveFocusedLane(delta: -1 | 1): void {
  const lanes = currentPlotLanes();
  if (lanes.length <= 1) return;
  const ids = lanes.map((l) => l.id ?? "");
  let idx = state.focusedLane ? ids.indexOf(state.focusedLane) : -1;
  if (idx < 0 && state.solo) {
    idx = lanes.findIndex((l) => l.traces.some((t) => t.name === state.solo));
  }
  if (idx < 0) {
    setNotice("Click a lane label pill first to focus a lane, then drag or use Alt+↑/↓ to move it.");
    return;
  }
  const target = idx + delta;
  if (target < 0 || target >= ids.length) return;
  const insertAt = delta > 0 ? target + 1 : target;
  state.laneOrder = reorderLaneIds(ids, idx, insertAt);
  state.dirty = true;
  renderDirty();
  draw();
}

export function resetLaneOrder(): void {
  state.laneOrder = [];
  state.dirty = true;
  renderDirty();
  draw();
  setNotice("Lane order reset to default.");
}

export function setLaneMode(mode: LaneMode): void {
  if (state.laneMode === mode) return;
  state.laneMode = mode;
  state.focusedLane = null;
  state.dirty = true;
  renderDirty();
  if (mode === "split") {
    setNotice("Scope: Split tracks — click a lane label to focus, drag it vertically to reorder.");
  } else if (mode === "grouped") {
    setNotice("Scope: Grouped tracks — click T1/T2 in legend to group, drag lane labels to reorder.");
  } else {
    setNotice("Scope: Single overlay track.");
  }
  draw();
}

export function cycleLaneMode(): void {
  setLaneMode(nextLaneMode(state.laneMode));
}

export function toggleLaneLoupe(): void {
  state.laneLoupe = !state.laneLoupe;
  if (!state.laneLoupe) {
    cancelLaneLoupeDwell();
    state.laneLoupeOpen = false;
  }
  laneLoupeDismissed = false;
  setNotice(
    state.laneLoupe
      ? "Hover lane magnifier: ON — pause briefly over any lane for 2-level zoom (Esc to dismiss, V to toggle)."
      : "Hover lane magnifier: OFF (click Loupe or press V to re-enable).",
  );
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
  els.laneModeOverlay.addEventListener("click", () => setLaneMode("overlay"));
  els.laneModeSplit.addEventListener("click", () => setLaneMode("split"));
  els.laneModeGrouped.addEventListener("click", () => setLaneMode("grouped"));
  els.laneLoupe.addEventListener("click", () => toggleLaneLoupe());
  els.rateDown.addEventListener("click", () => stepRate(-1));
  els.rateUp.addEventListener("click", () => stepRate(1));
  els.rate.addEventListener("click", (event) => {
    if (event.shiftKey) stepRate(-1);
    else if (state.rate !== 1) resetRate();
    else stepRate(1);
  });
  els.rate.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    stepRate(-1);
  });
  els.rate.addEventListener(
    "wheel",
    (event) => {
      event.preventDefault();
      stepRate(event.deltaY < 0 ? 1 : -1);
    },
    { passive: false },
  );
  els.zoomIn.addEventListener("click", () => zoom(0.7));
  els.zoomOut.addEventListener("click", () => zoom(1.4));
  els.zoomAll.addEventListener("click", resetZoom);

  let panX = 0;
  let mode: "idle" | "pan" | "scrub" | "box" | "lane" = "idle";
  let boxStartT = 0;
  let boxStartX = 0;
  let laneDownIdx = -1;
  let laneDownY = 0;
  let laneDragging = false;
  let lastHoverPill = -1;

  els.plot.addEventListener("pointermove", (event) => {
    if (!state.summary) return;
    const view = activeView(state.summary);
    // Ignore tiny hand tremor while keyboard-stepping lanes with ArrowUp/ArrowDown so hoverT/hoverLane stay locked.
    if (
      mode === "idle" &&
      laneKeyLocked &&
      Math.hypot(event.clientX - laneKeyLockX, event.clientY - laneKeyLockY) <= 4
    ) {
      return;
    }
    laneKeyLocked = false;
    state.hoverX = event.clientX;
    state.hoverY = event.clientY;

    if (mode === "lane" && laneDownIdx >= 0) {
      if (!laneDragging && Math.abs(event.clientY - laneDownY) > 5) {
        laneDragging = true;
        els.plot.classList.add("is-dragging-lane");
      }
      if (laneDragging) {
        state.hoverT = null;
        const dropIdx = laneDropIndexAt(els.plot, event.clientY, currentPlotLanes());
        setLaneDragOverlay({ dragLaneIndex: laneDownIdx, dropLaneIndex: dropIdx, hoverPillIndex: -1 });
        requestDraw();
      }
      return;
    }

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
      requestDraw();
      return;
    }
    if (mode === "scrub") {
      const t = timeOnPlot(els.plot, event.clientX, view);
      state.hoverT = t;
      scrubTo(t, false);
      return;
    }

    const lanes = currentPlotLanes();
    const pillIdx = lanePillAt(els.plot, event.clientX, event.clientY, lanes);
    els.plot.classList.toggle("on-lane-pill", pillIdx >= 0);
    if (pillIdx !== lastHoverPill) {
      lastHoverPill = pillIdx;
      setLaneDragOverlay({ hoverPillIndex: pillIdx });
    }
    const nextLane = pillIdx >= 0 ? -1 : laneAt(els.plot, event.clientY, lanes);
    state.hoverT = pillIdx >= 0 ? null : timeOnPlot(els.plot, event.clientX, view);
    if (nextLane !== state.hoverLane) {
      laneLoupeDismissed = false;
      state.hoverLane = nextLane;
    } else if (
      laneLoupeDismissed &&
      Math.hypot(event.clientX - laneLoupeDismissX, event.clientY - laneLoupeDismissY) > 18
    ) {
      laneLoupeDismissed = false;
    }

    if (!state.laneLoupe || nextLane < 0 || laneLoupeDismissed) {
      cancelLaneLoupeDwell();
      state.laneLoupeOpen = false;
    } else if (!state.laneLoupeOpen) {
      // Lazy dwell: only open the magnifier after pausing briefly over a lane.
      if (!laneLoupeDwellTimer || Math.hypot(event.clientX - laneLoupeAnchorX, event.clientY - laneLoupeAnchorY) > 6) {
        laneLoupeAnchorX = event.clientX;
        laneLoupeAnchorY = event.clientY;
        cancelLaneLoupeDwell();
        laneLoupeDwellTimer = window.setTimeout(() => {
          laneLoupeDwellTimer = 0;
          if (mode !== "idle" || !state.laneLoupe || !state.summary || state.hoverT == null || state.hoverLane < 0) {
            return;
          }
          const focusT = state.hoverT;
          const liveView = activeView(state.summary);
          // Fetch full-resolution data BEFORE showing the magnifier so Frame 1 is final (no tentative settle).
          void ensureLaneLoupeData(liveView, focusT).then(() => {
            if (mode === "idle" && state.laneLoupe && !laneLoupeDismissed && state.hoverT != null && state.hoverLane >= 0) {
              state.laneLoupeOpen = true;
              requestDraw();
            }
          });
        }, LANE_LOUPE_DWELL_MS);
      }
    }
    requestDraw();
  });

  els.plot.addEventListener("pointerleave", () => {
    if (mode === "idle") {
      cancelLaneLoupeDwell();
      state.laneLoupeOpen = false;
      laneLoupeDismissed = false;
      laneKeyLocked = false;
      state.hoverT = null;
      state.hoverLane = -1;
      if (lastHoverPill !== -1) {
        lastHoverPill = -1;
        els.plot.classList.remove("on-lane-pill");
        setLaneDragOverlay({ hoverPillIndex: -1 });
      }
      requestDraw();
    }
  });

  els.plot.addEventListener("pointerdown", (event) => {
    if (!state.summary) return;
    cancelLaneLoupeDwell();
    state.laneLoupeOpen = false;
    const view = activeView(state.summary);
    if (event.button === 1 || event.altKey) {
      mode = "pan";
      panX = event.clientX;
      els.plot.setPointerCapture(event.pointerId);
      return;
    }
    if (event.button !== 0) return;

    // Check if clicking/dragging a lane's frosted title pill (focus or reorder)
    const pillIdx = lanePillAt(els.plot, event.clientX, event.clientY, currentPlotLanes());
    if (pillIdx >= 0 && !event.shiftKey) {
      mode = "lane";
      laneDownIdx = pillIdx;
      laneDownY = event.clientY;
      laneDragging = false;
      els.plot.setPointerCapture(event.pointerId);
      return;
    }

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
    if (mode === "lane") {
      const lanes = currentPlotLanes();
      if (laneDragging && laneDownIdx >= 0) {
        const dropIdx = laneDropIndexAt(els.plot, event.clientY, lanes);
        const ids = lanes.map((l) => l.id ?? "");
        state.laneOrder = reorderLaneIds(ids, laneDownIdx, dropIdx);
        state.dirty = true;
        renderDirty();
      } else if (laneDownIdx >= 0) {
        const laneId = lanes[laneDownIdx]?.id ?? null;
        state.solo = null;
        state.focusedLane = state.focusedLane === laneId ? null : laneId;
      }
      laneDownIdx = -1;
      laneDragging = false;
      mode = "idle";
      els.plot.classList.remove("is-dragging-lane");
      setLaneDragOverlay({ dragLaneIndex: -1, dropLaneIndex: null });
      draw();
      return;
    }

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
    laneDownIdx = -1;
    laneDragging = false;
    mode = "idle";
    els.plot.classList.remove("is-dragging-lane");
    setLaneDragOverlay({ dragLaneIndex: -1, dropLaneIndex: null });
    requestDraw();
  });

  els.plot.addEventListener("dblclick", (event) => {
    if (lanePillAt(els.plot, event.clientX, event.clientY, currentPlotLanes()) >= 0) return;
    resetZoom();
  });

  let timelineDraggingWindow = false;
  let timelineLastT = 0;
  let loupeHideTimer = 0;
  let loupeHovering = false;
  let loupeDragging = false;
  let loupeDraggingWindow = false;
  let loupeLastT = 0;

  const cancelLoupeHide = () => {
    window.clearTimeout(loupeHideTimer);
  };

  const scheduleLoupeHide = () => {
    window.clearTimeout(loupeHideTimer);
    loupeHideTimer = window.setTimeout(() => {
      if (loupeHovering || loupeDragging || timelineDraggingWindow) return;
      if (currentLoupe()) {
        setLoupeState(null);
        requestDraw();
      }
    }, 180);
  };

  const updateLoupeFromTimeline = (clientX: number) => {
    if (!state.summary) return;
    const domain = { t0: state.summary.tStartUs, t1: state.summary.tEndUs };
    const view = activeView(state.summary);
    const rect = els.timeline.getBoundingClientRect();
    if (!shouldShowLoupe(domain, view, rect.width)) {
      if (currentLoupe()) {
        setLoupeState(null);
        requestDraw();
      }
      return;
    }
    const t = timeAt(els.timeline, clientX, domain);
    const computed = computeLoupeDomain(domain, view, t);
    setLoupeState({ range: computed.range, factor: computed.factor, anchorX: clientX });
    requestDraw();
  };

  els.timeline.addEventListener("pointerdown", (event) => {
    if (!state.summary) return;
    els.timeline.setPointerCapture(event.pointerId);
    const domain = { t0: state.summary.tStartUs, t1: state.summary.tEndUs };
    const t = timeAt(els.timeline, event.clientX, domain);
    const view = activeView(state.summary);
    const rect = els.timeline.getBoundingClientRect();
    const total = Math.max(1, domain.t1 - domain.t0);
    // Generous hit-slop (±10px) around a thin zoomed-in cyan viewport box so it's easy to grab!
    const slopUs = (10 / Math.max(1, rect.width - 20)) * total;
    const zoomedIn = view.t1 - view.t0 < duration(state.summary) * 0.98;
    if (zoomedIn && t >= view.t0 - slopUs && t <= view.t1 + slopUs) {
      timelineDraggingWindow = true;
      timelineLastT = t;
    } else {
      timelineDraggingWindow = false;
      scrubTo(t, true);
    }
    updateLoupeFromTimeline(event.clientX);
  });

  els.timeline.addEventListener("pointermove", (event) => {
    if (!state.summary) return;
    cancelLoupeHide();
    const domain = { t0: state.summary.tStartUs, t1: state.summary.tEndUs };
    const t = timeAt(els.timeline, event.clientX, domain);
    if (event.buttons & 1) {
      if (timelineDraggingWindow) {
        const dt = t - timelineLastT;
        timelineLastT = t;
        panBy(dt);
      } else {
        scrubTo(t, true);
      }
    }
    updateLoupeFromTimeline(event.clientX);
  });

  els.timeline.addEventListener("pointerleave", () => {
    scheduleLoupeHide();
  });

  els.timeline.addEventListener("pointerup", () => {
    timelineDraggingWindow = false;
  });

  // Micro-map loupe interactions: domain stays frozen while hovering/dragging inside so fine adjustments stay rock-steady!
  els.timelineLoupe.addEventListener("pointerenter", () => {
    loupeHovering = true;
    cancelLoupeHide();
  });

  els.timelineLoupe.addEventListener("pointerleave", () => {
    loupeHovering = false;
    scheduleLoupeHide();
  });

  els.timelineLoupeCanvas.addEventListener("pointerdown", (event) => {
    const loupe = currentLoupe();
    if (!state.summary || !loupe) return;
    els.timelineLoupeCanvas.setPointerCapture(event.pointerId);
    loupeDragging = true;
    const t = timeAt(els.timelineLoupeCanvas, event.clientX, loupe.range);
    const view = activeView(state.summary);
    const rect = els.timelineLoupeCanvas.getBoundingClientRect();
    const loupeSpan = Math.max(1, loupe.range.t1 - loupe.range.t0);
    const slopUs = (10 / Math.max(1, rect.width - 20)) * loupeSpan;
    if (t >= view.t0 - slopUs && t <= view.t1 + slopUs) {
      loupeDraggingWindow = true;
      loupeLastT = t;
    } else {
      loupeDraggingWindow = false;
      scrubTo(t, false);
    }
  });

  els.timelineLoupeCanvas.addEventListener("pointermove", (event) => {
    const loupe = currentLoupe();
    if (!(event.buttons & 1) || !loupeDragging || !state.summary || !loupe) return;
    const t = timeAt(els.timelineLoupeCanvas, event.clientX, loupe.range);
    if (loupeDraggingWindow) {
      const dt = t - loupeLastT;
      loupeLastT = t;
      panBy(dt);
    } else {
      scrubTo(t, false);
    }
  });

  els.timelineLoupeCanvas.addEventListener("pointerup", () => {
    loupeDragging = false;
    loupeDraggingWindow = false;
    // Gently re-center the loupe around the newly positioned viewport once released
    const loupe = currentLoupe();
    if (state.summary && loupe) {
      const domain = { t0: state.summary.tStartUs, t1: state.summary.tEndUs };
      const view = activeView(state.summary);
      const mid = (view.t0 + view.t1) / 2;
      const computed = computeLoupeDomain(domain, view, mid);
      setLoupeState({ range: computed.range, factor: computed.factor, anchorX: loupe.anchorX });
      requestDraw();
    }
  });

  els.timelineLoupeCanvas.addEventListener("pointercancel", () => {
    loupeDragging = false;
    loupeDraggingWindow = false;
  });

  els.timelineLoupeCanvas.addEventListener(
    "wheel",
    (event) => {
      const loupe = currentLoupe();
      if (!state.summary || !loupe) return;
      event.preventDefault();
      if (event.shiftKey) {
        panBy((event.deltaY / 400) * state.span);
        return;
      }
      const anchor = timeAt(els.timelineLoupeCanvas, event.clientX, loupe.range);
      const factor = Math.exp(event.deltaY * 0.0012);
      zoom(factor, anchor);
      const domain = { t0: state.summary.tStartUs, t1: state.summary.tEndUs };
      const view = activeView(state.summary);
      const rect = els.timeline.getBoundingClientRect();
      if (!shouldShowLoupe(domain, view, rect.width)) {
        setLoupeState(null);
      } else {
        const computed = computeLoupeDomain(domain, view, anchor);
        setLoupeState({ range: computed.range, factor: computed.factor, anchorX: loupe.anchorX });
      }
      requestDraw();
    },
    { passive: false },
  );

  els.plot.addEventListener("wheel", onWheel, { passive: false });
  els.timeline.addEventListener("wheel", onWheel, { passive: false });
}
