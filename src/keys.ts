import * as api from "./api";
import { els } from "./dom";
import { loadSample, openLog, openProject, saveProject } from "./files";
import { hasData } from "./model";
import { bindPalette, closePalette, openPalette, type CommandItem } from "./palette";
import { addNewWidget } from "./panels/cluster";
import { openMathEditor } from "./panels/deck";
import { dropCursor } from "./panels/drive";
import { addBookmark, removeBookmark } from "./panels/marks";
import { peekRail, renderSignals, toggleClusterStrip, toggleHousekeeping, toggleRail } from "./panels/signals";
import { showTab } from "./panels/tabs";
import { refresh, refreshCursors, refreshOverview, setPlotted } from "./query";
import { renderDirty } from "./readout";
import { draw, renderChrome, state } from "./state";
import { setNotice } from "./status";
import {
  centerOnPlayhead,
  cycleLaneMode,
  dismissLaneLoupe,
  moveFocusedLane,
  panBy,
  resetLaneOrder,
  resetRate,
  resetZoom,
  scrubTo,
  setLaneMode,
  stepEvent,
  stepFrame,
  stepHoverLane,
  stepRate,
  toggleLaneLoupe,
  togglePlay,
  zoom,
  zoomToCursors,
} from "./transport";

function typingTarget(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement
  );
}

function clearCursors(): void {
  state.cursorA = null;
  state.cursorB = null;
  state.dirty = true;
  void refreshCursors();
  draw();
}

function focusSignalFilter(): void {
  peekRail();
  els.sigFilter.focus();
  els.sigFilter.select();
}

function buildCommands(): CommandItem[] {
  const cmds: CommandItem[] = [
    // Navigation & Playback
    {
      id: "play",
      title: state.playing ? "Pause playback" : "Play / Pause",
      category: "Playback & Navigation",
      shortcut: ["Space"],
      keywords: "toggle start stop transport",
      run: () => togglePlay(),
    },
    {
      id: "step-next",
      title: "Next frame",
      category: "Playback & Navigation",
      shortcut: ["→", "."],
      keywords: "step forward single frame",
      run: () => void stepFrame("next"),
    },
    {
      id: "step-prev",
      title: "Previous frame",
      category: "Playback & Navigation",
      shortcut: ["←", ","],
      keywords: "step backward single frame",
      run: () => void stepFrame("prev"),
    },
    {
      id: "event-next",
      title: "Next event or bookmark",
      category: "Playback & Navigation",
      shortcut: ["Shift", "→"],
      keywords: "jump fault trigger mark",
      run: () => stepEvent(1),
    },
    {
      id: "event-prev",
      title: "Previous event or bookmark",
      category: "Playback & Navigation",
      shortcut: ["Shift", "←"],
      keywords: "jump fault trigger mark",
      run: () => stepEvent(-1),
    },
    {
      id: "rate-up",
      title: `Speed up playback (current ${state.rate}×)`,
      category: "Playback & Navigation",
      shortcut: [">", "R"],
      keywords: "faster increase speed 2x 4x 8x 16x",
      run: () => stepRate(1),
    },
    {
      id: "rate-down",
      title: `Slow down playback (current ${state.rate}×)`,
      category: "Playback & Navigation",
      shortcut: ["<", "Shift+R"],
      keywords: "slower decrease slow motion 0.5x 0.25x 0.1x",
      run: () => stepRate(-1),
    },
    {
      id: "rate-reset",
      title: "Reset playback speed to 1×",
      category: "Playback & Navigation",
      keywords: "normal realtime speed",
      run: () => resetRate(),
    },
    {
      id: "start",
      title: "Jump to start of log",
      category: "Playback & Navigation",
      shortcut: ["Home"],
      keywords: "beginning first frame",
      run: () => {
        if (state.summary) scrubTo(state.summary.tStartUs);
      },
    },
    {
      id: "end",
      title: "Jump to end of log",
      category: "Playback & Navigation",
      shortcut: ["End"],
      keywords: "last frame finish",
      run: () => {
        if (state.summary) scrubTo(state.summary.tEndUs);
      },
    },
    {
      id: "center",
      title: "Center camera on playhead",
      category: "Playback & Navigation",
      shortcut: ["G"],
      keywords: "follow track recenter cursor",
      run: () => centerOnPlayhead(),
    },

    // Zoom & Pan (Perfetto / Saleae WASD + standard +/-)
    {
      id: "zoom-in",
      title: "Zoom in",
      category: "Zoom & Pan",
      shortcut: ["W", "=", "]"],
      keywords: "magnify narrower span closer",
      run: () => zoom(0.7),
    },
    {
      id: "zoom-out",
      title: "Zoom out",
      category: "Zoom & Pan",
      shortcut: ["S", "−", "["],
      keywords: "wider span farther",
      run: () => zoom(1.4),
    },
    {
      id: "pan-left",
      title: "Pan camera left",
      category: "Zoom & Pan",
      shortcut: ["A"],
      keywords: "scroll earlier backward",
      run: () => panBy(-state.span * 0.2),
    },
    {
      id: "pan-right",
      title: "Pan camera right",
      category: "Zoom & Pan",
      shortcut: ["D"],
      keywords: "scroll later forward",
      run: () => panBy(state.span * 0.2),
    },
    {
      id: "zoom-fit",
      title: "Zoom to fit entire log",
      category: "Zoom & Pan",
      shortcut: ["F", "0"],
      keywords: "reset all full view",
      run: () => resetZoom(),
    },
    {
      id: "zoom-cursors",
      title: "Zoom to Cursors A–B",
      category: "Zoom & Pan",
      shortcut: ["Z"],
      keywords: "selection range fit cursors",
      run: () => zoomToCursors(),
    },

    // Layout & Workspace
    {
      id: "toggle-rail",
      title: state.railCollapsed ? "Pin signal sidebar open" : "Hide signal sidebar (hover to peek)",
      category: "Layout & View",
      shortcut: ["Ctrl", "B"],
      keywords: "collapse rail panel width landscape",
      run: () => toggleRail(),
    },
    {
      id: "lane-mode",
      title: "Cycle track layout (Overlay / Split / Group)",
      category: "Layout & View",
      shortcut: ["L"],
      keywords: "lanes stacked tracks separate group",
      run: () => cycleLaneMode(),
    },
    {
      id: "lane-mode-overlay",
      title: "Track layout: Overlay",
      category: "Layout & View",
      keywords: "single track combined overlay",
      run: () => setLaneMode("overlay"),
    },
    {
      id: "lane-mode-split",
      title: "Track layout: Split",
      category: "Layout & View",
      keywords: "separate lane per signal split",
      run: () => setLaneMode("split"),
    },
    {
      id: "lane-mode-grouped",
      title: "Track layout: Group",
      category: "Layout & View",
      keywords: "group unit message t1 t2",
      run: () => setLaneMode("grouped"),
    },
    ...(Object.keys(state.signalColors).length > 0
      ? [
          {
            id: "reset-signal-colors",
            title: "Reset all custom signal colors",
            category: "Layout & View",
            keywords: "palette auto default colors",
            run: () => {
              state.signalColors = {};
              state.dirty = true;
              renderDirty();
              renderSignals();
              draw();
              setNotice("Signal colors reset to automatic defaults.");
            },
          },
        ]
      : []),
    {
      id: "lane-loupe",
      title: state.laneLoupe ? "Hide lane hover magnifier" : "Show lane hover magnifier",
      category: "Layout & View",
      shortcut: ["V"],
      keywords: "loupe zoom micro map inspect hover",
      run: () => toggleLaneLoupe(),
    },
    {
      id: "lane-loupe-step",
      title: "Switch magnified lane up / down at locked time",
      category: "Layout & View",
      shortcut: ["↑", "↓"],
      keywords: "loupe compare lane group hover inspect",
      run: () => {
        stepHoverLane(1);
      },
    },
    {
      id: "add-gadget",
      title: "Add cluster gadget",
      category: "Layout & View",
      shortcut: ["+"],
      keywords: "widget dial bar lamp readout gauge dashboard",
      run: () => addNewWidget("value"),
    },
    {
      id: "toggle-cluster",
      title: state.clusterHidden ? "Show cluster gadgets bar" : "Hide cluster gadgets bar",
      category: "Layout & View",
      shortcut: ["Shift", "G"],
      keywords: "gauges dashboard vertical space",
      run: () => toggleClusterStrip(),
    },
    {
      id: "toggle-chk",
      title: state.hideHousekeeping ? "Show checksum & counter signals in list" : "Hide checksum & counter signals in list",
      category: "Layout & View",
      keywords: "crc chk counter alive housekeeping filter",
      run: () => toggleHousekeeping(),
    },
    {
      id: "filter-signals",
      title: "Filter signals",
      category: "Layout & View",
      shortcut: ["/", "Ctrl+F"],
      keywords: "search find signal",
      run: () => focusSignalFilter(),
    },
    {
      id: "clear-solo",
      title: "Clear lane / signal focus",
      category: "Layout & View",
      shortcut: ["Esc"],
      keywords: "unsolo unfocus dim show all",
      run: () => {
        if (state.solo || state.focusedLane) {
          state.solo = null;
          state.focusedLane = null;
          draw();
        }
      },
    },
    {
      id: "lane-up",
      title: "Move focused lane up",
      category: "Layout & View",
      shortcut: ["Alt", "↑"],
      keywords: "reorder arrange track group",
      run: () => moveFocusedLane(-1),
    },
    {
      id: "lane-down",
      title: "Move focused lane down",
      category: "Layout & View",
      shortcut: ["Alt", "↓"],
      keywords: "reorder arrange track group",
      run: () => moveFocusedLane(1),
    },
    {
      id: "lane-order-reset",
      title: "Reset lane order",
      category: "Layout & View",
      keywords: "default arrange tracks",
      run: () => resetLaneOrder(),
    },
    ...(api.inTauri()
      ? [
          {
            id: "win-max",
            title: "Maximize / Restore window",
            category: "Layout & View",
            shortcut: ["F11"],
            keywords: "fullscreen window size expand",
            run: () => {
              void import("@tauri-apps/api/window").then(({ getCurrentWindow }) =>
                getCurrentWindow().toggleMaximize(),
              );
            },
          },
        ]
      : []),

    // Markers, Cursors & Math
    {
      id: "math-new",
      title: "New math channel (Studio)",
      category: "Analysis & Markers",
      shortcut: ["Shift", "M"],
      keywords: "derived formula expression builder lp deriv",
      run: () => {
        showTab("math");
        openMathEditor(null);
      },
    },
    {
      id: "bookmark",
      title: "Drop bookmark at playhead",
      category: "Analysis & Markers",
      shortcut: ["M", "B"],
      keywords: "mark pin flag",
      run: () => addBookmark(),
    },
    {
      id: "note",
      title: "Add note at playhead",
      category: "Analysis & Markers",
      shortcut: ["N"],
      keywords: "comment annotation",
      run: () => {
        peekRail();
        showTab("notes");
        els.noteBody.focus();
      },
    },
    {
      id: "cursor-a",
      title: "Drop Cursor A at playhead",
      category: "Analysis & Markers",
      shortcut: ["1"],
      keywords: "measure delta start",
      run: () => dropCursor("a"),
    },
    {
      id: "cursor-b",
      title: "Drop Cursor B at playhead",
      category: "Analysis & Markers",
      shortcut: ["2"],
      keywords: "measure delta end",
      run: () => dropCursor("b"),
    },
    {
      id: "cursor-clear",
      title: "Clear Cursors A & B",
      category: "Analysis & Markers",
      shortcut: ["3"],
      keywords: "remove reset measure",
      run: () => clearCursors(),
    },

    // Files & Tabs
    {
      id: "open-log",
      title: "Open CAN / telemetry log",
      category: "Files",
      shortcut: ["Ctrl", "O"],
      keywords: "file asc blf slog csv candump",
      run: () => void openLog(),
    },
    {
      id: "open-project",
      title: "Open .loom project",
      category: "Files",
      shortcut: ["Ctrl", "Shift", "O"],
      keywords: "load workspace session",
      run: () => void openProject(),
    },
    {
      id: "save-project",
      title: "Save project",
      category: "Files",
      shortcut: ["Ctrl", "S"],
      keywords: "write loom",
      run: () => void saveProject(false),
    },
    {
      id: "save-project-as",
      title: "Save project as…",
      category: "Files",
      shortcut: ["Ctrl", "Shift", "S"],
      keywords: "export copy loom",
      run: () => void saveProject(true),
    },
    {
      id: "load-sample",
      title: "Load built-in cluster sample",
      category: "Files",
      shortcut: ["Ctrl", "Shift", "L"],
      keywords: "demo synthetic drive",
      run: () => void loadSample(),
    },
    {
      id: "tab-marks",
      title: "Show Marks & Events tab",
      category: "Panels",
      keywords: "bookmarks dtc faults",
      run: () => {
        peekRail();
        showTab("marks");
      },
    },
    {
      id: "tab-math",
      title: "Show Math channels tab",
      category: "Panels",
      keywords: "derived expressions",
      run: () => {
        peekRail();
        showTab("math");
      },
    },
    {
      id: "tab-alerts",
      title: "Show Alerts & Triggers tab",
      category: "Panels",
      keywords: "threshold timeout",
      run: () => {
        peekRail();
        showTab("alerts");
      },
    },
    {
      id: "tab-drive",
      title: "Show Drive Compare & Export tab",
      category: "Panels",
      keywords: "csv slog compare socketcan",
      run: () => {
        peekRail();
        showTab("drive");
      },
    },
  ];

  // Also include live signals so users can fuzzy-toggle any signal right from Ctrl+K!
  if (state.summary) {
    const active = state.summary.signals.filter(hasData).slice(0, 200);
    for (const sig of active) {
      const isPlotted = state.plotted.includes(sig.name);
      cmds.push({
        id: `sig:${sig.name}`,
        title: `${isPlotted ? "Hide" : "Plot"} signal: ${sig.name}`,
        category: "Signals",
        keywords: `${sig.unit} ${sig.messageName} toggle trace`,
        run: () => {
          if (state.plotted.includes(sig.name)) {
            setPlotted(state.plotted.filter((n) => n !== sig.name));
          } else {
            setPlotted([...state.plotted, sig.name]);
          }
          state.dirty = true;
          renderChrome();
          draw();
          void refresh();
          void refreshOverview();
        },
      });
    }
  }

  return cmds;
}

function onKey(event: KeyboardEvent): void {
  const mod = event.metaKey || event.ctrlKey;
  const lower = event.key.toLowerCase();

  // Command palette & shortcuts finder (Ctrl/Cmd+K or Ctrl/Cmd+Shift+P)
  if ((mod && lower === "k") || (mod && event.shiftKey && lower === "p")) {
    event.preventDefault();
    if (els.palette.open) closePalette();
    else openPalette();
    return;
  }
  if (mod && lower === "b" && !event.shiftKey) {
    event.preventDefault();
    toggleRail();
    return;
  }
  if (mod && lower === "f" && !event.shiftKey) {
    event.preventDefault();
    focusSignalFilter();
    return;
  }

  if (event.key === "Escape") {
    if (!els.widgetEditor.hidden || !els.colorMenu.hidden || !els.styleMenu.hidden) return;
    if (state.laneLoupeOpen) {
      dismissLaneLoupe();
      return;
    }
    closePalette();
    els.sigFilter.blur();
    if (state.solo || state.focusedLane) {
      state.solo = null;
      state.focusedLane = null;
      draw();
    }
    return;
  }
  if (event.key === "F11" && api.inTauri()) {
    event.preventDefault();
    void import("@tauri-apps/api/window").then(({ getCurrentWindow }) => getCurrentWindow().toggleMaximize());
    return;
  }
  if (mod && lower === "o" && !event.shiftKey) {
    event.preventDefault();
    void openLog();
    return;
  }
  if (mod && lower === "o" && event.shiftKey) {
    event.preventDefault();
    void openProject();
    return;
  }
  if (mod && lower === "s") {
    event.preventDefault();
    void saveProject(event.shiftKey);
    return;
  }
  if (mod && event.shiftKey && lower === "l") {
    event.preventDefault();
    void loadSample();
    return;
  }

  if (els.palette.open || els.mathDialog.open || !els.widgetEditor.hidden) return;
  if (typingTarget(event.target)) return;

  if (event.key === "?" || (event.shiftKey && event.key === "/")) {
    event.preventDefault();
    openPalette();
    return;
  }
  if (event.key === "/") {
    event.preventDefault();
    focusSignalFilter();
    return;
  }
  if (event.key === " ") {
    event.preventDefault();
    togglePlay();
    return;
  }
  if (!mod && lower === "l" && !event.shiftKey) {
    event.preventDefault();
    cycleLaneMode();
    return;
  }
  if (!mod && event.shiftKey && lower === "g") {
    event.preventDefault();
    toggleClusterStrip();
    return;
  }
  if (!mod && !event.shiftKey && lower === "g") {
    event.preventDefault();
    centerOnPlayhead();
    return;
  }
  if (!mod && event.shiftKey && lower === "m") {
    event.preventDefault();
    showTab("math");
    openMathEditor(null);
    return;
  }
  if (!mod && !event.shiftKey && lower === "v") {
    event.preventDefault();
    toggleLaneLoupe();
    return;
  }
  if (!mod && (event.key === "+" || event.code === "NumpadAdd")) {
    event.preventDefault();
    addNewWidget("value");
    return;
  }
  if (!mod && !event.shiftKey && (lower === "w" || event.key === "=" || event.key === "]")) {
    event.preventDefault();
    zoom(0.7);
    return;
  }
  if (!mod && !event.shiftKey && (lower === "s" || event.key === "-" || event.key === "[")) {
    event.preventDefault();
    zoom(1.4);
    return;
  }
  if (!mod && !event.shiftKey && lower === "a") {
    event.preventDefault();
    panBy(-state.span * 0.2);
    return;
  }
  if (!mod && !event.shiftKey && lower === "d") {
    event.preventDefault();
    panBy(state.span * 0.2);
    return;
  }
  if (!mod && !event.shiftKey && (lower === "f" || event.key === "0")) {
    event.preventDefault();
    resetZoom();
    return;
  }
  if (!mod && !event.shiftKey && lower === "z") {
    event.preventDefault();
    zoomToCursors();
    return;
  }
  if (!mod && event.altKey && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
    event.preventDefault();
    moveFocusedLane(event.key === "ArrowUp" ? -1 : 1);
    return;
  }
  if (!mod && !event.altKey && !event.shiftKey && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
    if (stepHoverLane(event.key === "ArrowUp" ? -1 : 1)) {
      event.preventDefault();
      return;
    }
  }
  if (!mod && !event.shiftKey && lower === "r") {
    event.preventDefault();
    stepRate(1);
    return;
  }
  if (!mod && event.shiftKey && lower === "r") {
    event.preventDefault();
    stepRate(-1);
    return;
  }
  if (!mod && (event.key === ">" || (event.shiftKey && event.key === "."))) {
    event.preventDefault();
    stepRate(1);
    return;
  }
  if (!mod && (event.key === "<" || (event.shiftKey && event.key === ","))) {
    event.preventDefault();
    stepRate(-1);
    return;
  }
  if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
    event.preventDefault();
    const dir = event.key === "ArrowRight" ? 1 : -1;
    if (event.shiftKey) stepEvent(dir as 1 | -1);
    else void stepFrame(dir > 0 ? "next" : "prev");
    return;
  }
  if (!mod && !event.shiftKey && (event.key === "," || event.key === ".")) {
    event.preventDefault();
    void stepFrame(event.key === "." ? "next" : "prev");
    return;
  }
  if (!mod && !event.shiftKey && (lower === "b" || lower === "m")) {
    event.preventDefault();
    addBookmark();
    return;
  }
  if (!mod && !event.shiftKey && lower === "n") {
    event.preventDefault();
    peekRail();
    showTab("notes");
    els.noteBody.focus();
    return;
  }
  if (event.key === "1") {
    event.preventDefault();
    dropCursor("a");
    return;
  }
  if (event.key === "2") {
    event.preventDefault();
    dropCursor("b");
    return;
  }
  if (event.key === "3") {
    event.preventDefault();
    clearCursors();
    return;
  }
  if ((event.key === "Delete" || event.key === "Backspace") && state.selectedMark) {
    event.preventDefault();
    removeBookmark(state.selectedMark);
    return;
  }
  if (event.key === "Home" && state.summary) {
    event.preventDefault();
    scrubTo(state.summary.tStartUs);
  }
  if (event.key === "End" && state.summary) {
    event.preventDefault();
    scrubTo(state.summary.tEndUs);
  }
}

export function bindKeys(): void {
  bindPalette(buildCommands);
  els.btnHelp.addEventListener("click", () => openPalette());
  window.addEventListener("keydown", onKey);
}
