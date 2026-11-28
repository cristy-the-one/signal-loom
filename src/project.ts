import { defaultClusterWidgets, serializeClusterWidgets, type ClusterBindings } from "./gauges";
import { els } from "./dom";
import { clamp } from "./format";
import { LANE_MODES, type LaneMode } from "./lanes";
import { MIN_SPAN, defaultPlotted, duration, hasData } from "./model";
import { setClusterHidden, setRailCollapsed } from "./panels/signals";
import { refresh, refreshCursors, refreshOverview } from "./query";
import { bumpDataGen, renderChrome, state } from "./state";
import { clearError, setError } from "./status";
import type { ProjectOpen, ProjectView, Summary, TraceStyle } from "./types";

function isLaneMode(value: unknown): value is LaneMode {
  return typeof value === "string" && (LANE_MODES as readonly string[]).includes(value);
}

/** Forgets everything the deck holds for the old log: math, triggers, notes, cursors, compare drive, readings. */
export function resetDeck(): void {
  state.math = [];
  state.triggers = [];
  state.notes = [];
  state.cursorA = null;
  state.cursorB = null;
  state.cursorText = "";
  state.compareOn = false;
  state.comparePath = null;
  state.compareOffsetUs = 0;
  state.viewCenter = null;
  state.boxZoom = null;
  state.solo = null;
  state.focusedLane = null;
  state.laneOrder = [];
  state.signalLanes = {};
  state.signalStyles = {};
  state.signalColors = {};
  state.held = [];
  state.bus = null;
  els.compareOffset.value = "0";
}

export function adoptSummary(summary: Summary, mode: "fresh" | "keep"): void {
  bumpDataGen();
  state.summary = summary;
  els.timeoutFactor.value = String(summary.timeoutFactor);
  if (mode === "fresh") {
    state.bookmarks = [];
    state.selectedMark = null;
    state.projectPath = null;
    state.playhead = summary.tStartUs;
    state.span = duration(summary);
    state.plotted = defaultPlotted(summary);
    state.dirty = false;
    state.overview = null;
    state.overviewName = null;
    resetDeck();
  } else {
    state.plotted = state.plotted.filter((name) => summary.signals.some((signal) => signal.name === name));
    if (state.solo && !state.plotted.includes(state.solo)) state.solo = null;
    const plottedWithData = summary.signals.some((signal) => hasData(signal) && state.plotted.includes(signal.name));
    if (!plottedWithData) state.plotted = defaultPlotted(summary);
    state.playhead = clamp(state.playhead, summary.tStartUs, summary.tEndUs);
    if (state.viewCenter != null) {
      state.viewCenter = clamp(state.viewCenter, summary.tStartUs, summary.tEndUs);
    }
    state.span = clamp(state.span, MIN_SPAN, duration(summary));
    state.overview = null;
    state.overviewName = null;
  }
  clearError("action");
  clearError("query");
  if (mode === "fresh") clearError("project");
  renderChrome();
  void refresh();
  void refreshOverview();
  void refreshCursors();
}

/** Adopts a summary the user's own edit produced (a map, a deck change): the project now has unsaved changes. */
export function adoptEdited(summary: Summary): void {
  adoptSummary(summary, "keep");
  state.dirty = true;
  renderChrome();
}

export function applyProject(opened: ProjectOpen, path: string | null): void {
  bumpDataGen();
  state.summary = opened.summary;
  state.bookmarks = opened.project.bookmarks;
  state.playhead = opened.project.view.playheadUs;
  state.viewCenter = null;
  state.boxZoom = null;
  state.solo = null;
  state.focusedLane = null;
  state.laneMode = isLaneMode(opened.project.laneMode) ? opened.project.laneMode : "overlay";
  state.laneOrder = [...(opened.project.laneOrder ?? [])];
  state.signalLanes = { ...(opened.project.signalLanes ?? {}) };
  state.signalColors = { ...(opened.project.signalColors ?? {}) };
  state.span = opened.project.view.spanUs || duration(opened.summary);
  state.plotted = opened.project.view.plotted.filter((name) =>
    opened.summary.signals.some((signal) => signal.name === name),
  );
  if (state.plotted.length === 0) state.plotted = defaultPlotted(opened.summary);
  state.playhead = clamp(state.playhead, opened.summary.tStartUs, opened.summary.tEndUs);
  state.span = clamp(state.span, MIN_SPAN, duration(opened.summary));
  state.projectPath = path;
  state.dirty = false;
  state.selectedMark = null;
  state.overview = null;
  state.overviewName = null;
  state.math = opened.project.math ?? [];
  state.triggers = opened.project.triggers ?? [];
  state.notes = opened.project.notes ?? [];
  state.cursorA = opened.project.cursorAUs ?? null;
  state.cursorB = opened.project.cursorBUs ?? null;
  state.comparePath = opened.project.comparePath ?? null;
  state.compareOffsetUs = opened.project.compareOffsetUs ?? 0;
  state.cluster = (opened.project.cluster ?? {}) as ClusterBindings;
  const hasSavedWidgets =
    Boolean(opened.project.clusterCustomized) ||
    (opened.project.clusterWidgets != null && opened.project.clusterWidgets.length > 0);
  if (hasSavedWidgets) {
    state.clusterWidgets = (opened.project.clusterWidgets ?? []).map((w) => ({ ...w }));
    state.clusterCustomized = true;
  } else {
    state.clusterWidgets = defaultClusterWidgets(state.cluster);
    state.clusterCustomized = Object.keys(state.cluster).length > 0;
  }
  state.signalStyles = { ...(opened.project.signalStyles ?? {}) };
  state.hideHousekeeping = Boolean(opened.project.hideHousekeeping);
  setRailCollapsed(Boolean(opened.project.railCollapsed), false);
  setClusterHidden(Boolean(opened.project.clusterHidden), false);
  state.compareOn = opened.compareOpened;
  els.compareOffset.value = String(state.compareOffsetUs / 1000);
  clearError();
  if (opened.warnings.length) setError(opened.warnings.join(" "), "project");
  void refreshCursors();
  renderChrome();
  void refresh();
  void refreshOverview();
}

/** What the UI owns of the project. The engine fills in the deck it holds when it saves. */
export function currentView(): ProjectView {
  const signalStyles: Record<string, TraceStyle> = {};
  for (const [name, style] of Object.entries(state.signalStyles)) {
    if (style && style !== "auto") signalStyles[name] = style;
  }
  const signalColors: Record<string, string> = {};
  for (const [name, color] of Object.entries(state.signalColors)) {
    if (color && /^#[0-9a-f]{3,8}$/i.test(color.trim())) signalColors[name] = color.trim();
  }
  const signalLanes: Record<string, number> = {};
  for (const [name, lane] of Object.entries(state.signalLanes)) {
    if (Number.isInteger(lane) && lane > 0) signalLanes[name] = lane;
  }
  return {
    bookmarks: state.bookmarks,
    view: {
      playheadUs: Math.round(state.playhead),
      spanUs: Math.round(state.span),
      plotted: [...state.plotted],
    },
    notes: state.notes,
    cursorAUs: state.cursorA,
    cursorBUs: state.cursorB,
    cluster: state.cluster as Record<string, string>,
    clusterWidgets: state.clusterCustomized ? serializeClusterWidgets(state.clusterWidgets) : [],
    clusterCustomized: state.clusterCustomized,
    signalStyles,
    signalColors,
    laneMode: state.laneMode,
    signalLanes,
    laneOrder: [...state.laneOrder],
    railCollapsed: state.railCollapsed,
    clusterHidden: state.clusterHidden,
    hideHousekeeping: state.hideHousekeeping,
  };
}
