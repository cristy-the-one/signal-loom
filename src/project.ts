import type { ClusterBindings } from "./gauges";
import { els } from "./dom";
import { clamp } from "./format";
import { MIN_SPAN, defaultPlotted, duration, hasData } from "./model";
import { refresh, refreshCursors, refreshOverview } from "./query";
import { bumpDataGen, renderChrome, state } from "./state";
import { clearError, setError } from "./status";
import type { ProjectOpen, ProjectView, Summary } from "./types";

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
    const plottedWithData = summary.signals.some((signal) => hasData(signal) && state.plotted.includes(signal.name));
    if (!plottedWithData) state.plotted = defaultPlotted(summary);
    state.playhead = clamp(state.playhead, summary.tStartUs, summary.tEndUs);
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
  };
}
