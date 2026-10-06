import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import * as api from "./api";
import { TRACE_COLORS, drawPlot, type Trace } from "./plot";
import { drawTimeline, timeAt, type TimelineMark } from "./timeline";
import {
  basename,
  clamp,
  errText,
  formatBytes,
  formatCount,
  formatSpan,
  formatUs,
  formatValue,
  hexId,
} from "./format";
import type { Bookmark, FrameHit, Point, ProjectFile, ProjectOpen, Series, Summary } from "./types";
import "./styles.css";

const MIN_SPAN = 10_000;
const RATES = [1, 4, 16];

const els = {
  projectName: must<HTMLElement>("project-name"),
  logName: must<HTMLElement>("log-name"),
  logMeta: must<HTMLElement>("log-meta"),
  runtime: must<HTMLElement>("runtime"),
  error: must<HTMLElement>("error"),
  sigCount: must<HTMLElement>("sig-count"),
  sigFilter: must<HTMLInputElement>("sig-filter"),
  sigList: must<HTMLElement>("sig-list"),
  markForm: must<HTMLFormElement>("mark-form"),
  markLabel: must<HTMLInputElement>("mark-label"),
  markList: must<HTMLElement>("mark-list"),
  eventList: must<HTMLElement>("event-list"),
  viewport: must<HTMLElement>("viewport"),
  plot: must<HTMLCanvasElement>("plot"),
  timeline: must<HTMLCanvasElement>("timeline"),
  legend: must<HTMLElement>("legend"),
  scaleNote: must<HTMLElement>("scale-note"),
  stageMsg: must<HTMLElement>("stage-msg"),
  timeReadout: must<HTMLElement>("time-readout"),
  frameReadout: must<HTMLElement>("frame-readout"),
  spanReadout: must<HTMLElement>("span-readout"),
  play: must<HTMLButtonElement>("play"),
  rate: must<HTMLButtonElement>("rate"),
  veil: must<HTMLElement>("veil"),
  veilLabel: must<HTMLElement>("veil-label"),
  drop: must<HTMLElement>("drop"),
  help: must<HTMLDialogElement>("help"),
  save: must<HTMLButtonElement>("btn-save"),
  fileLog: must<HTMLInputElement>("file-log"),
  fileMap: must<HTMLInputElement>("file-map"),
  fileProject: must<HTMLInputElement>("file-project"),
};

interface View {
  t0: number;
  t1: number;
}

const state: {
  summary: Summary | null;
  playhead: number;
  span: number;
  plotted: string[];
  bookmarks: Bookmark[];
  selectedMark: string | null;
  filter: string;
  projectPath: string | null;
  dirty: boolean;
  playing: boolean;
  rate: number;
  busy: number;
  series: Series[];
  view: View;
  frame: FrameHit | null;
  overview: Point[] | null;
  overviewName: string | null;
} = {
  summary: null,
  playhead: 0,
  span: 1,
  plotted: [],
  bookmarks: [],
  selectedMark: null,
  filter: "",
  projectPath: null,
  dirty: false,
  playing: false,
  rate: 1,
  busy: 0,
  series: [],
  view: { t0: 0, t1: 1 },
  frame: null,
  overview: null,
  overviewName: null,
};

let queryToken = 0;
let queryFlight = false;
let queryAgain = false;
let refreshTimer = 0;
let lastFetch = 0;
let playStamp = 0;

function must<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`missing #${id}`);
  return node as T;
}

function duration(summary: Summary): number {
  return Math.max(1, summary.tEndUs - summary.tStartUs);
}

function defaultPlotted(summary: Summary): string[] {
  const names = new Set(summary.signals.map((signal) => signal.name));
  const preferred = ["VehicleSpeed", "EngineRPM", "BrakePressure"].filter((name) => names.has(name));
  if (preferred.length) return preferred;
  return summary.signals.slice(0, 3).map((signal) => signal.name);
}

function colorFor(name: string): string {
  const index = state.summary?.signals.findIndex((signal) => signal.name === name) ?? 0;
  return TRACE_COLORS[(index < 0 ? 0 : index) % TRACE_COLORS.length];
}

function windowFor(playhead: number, span: number, summary: Summary): View {
  const start = summary.tStartUs;
  const end = summary.tEndUs;
  const dur = duration(summary);
  const width = clamp(span, MIN_SPAN, dur);
  if (width >= dur - 0.5) return { t0: start, t1: end };
  let a = playhead - width / 2;
  let b = a + width;
  if (a < start) {
    a = start;
    b = start + width;
  }
  if (b > end) {
    b = end;
    a = end - width;
  }
  return { t0: a, t1: b };
}

function setError(message: string): void {
  els.error.hidden = false;
  els.error.textContent = message;
}

function clearError(): void {
  els.error.hidden = true;
  els.error.textContent = "";
}

function paintBusy(label?: string): void {
  const active = state.busy > 0;
  els.viewport.classList.toggle("is-busy", active);
  els.veil.hidden = !active;
  if (label) els.veilLabel.textContent = label;
}

async function withBusy(label: string, work: () => Promise<void>): Promise<void> {
  state.busy += 1;
  paintBusy(label);
  await new Promise((resolve) => requestAnimationFrame(() => resolve(undefined)));
  try {
    await work();
  } catch (err) {
    setError(errText(err));
  } finally {
    state.busy = Math.max(0, state.busy - 1);
    paintBusy();
  }
}

function renderChrome(): void {
  const summary = state.summary;
  const project = state.projectPath ? basename(state.projectPath) : "Untitled";
  els.projectName.textContent = state.dirty ? `${project} ·` : project;
  document.title = summary ? `Signal Loom — ${summary.logLabel}` : "Signal Loom";
  els.save.classList.toggle("is-dirty", state.dirty);
  if (!summary) {
    els.logName.textContent = "No log";
    els.logMeta.textContent = "Open a recording to index it";
    els.sigCount.textContent = "0";
    return;
  }
  els.logName.textContent = summary.logLabel;
  const map = summary.mapLabel ?? "no signal map";
  els.logMeta.textContent = `${formatCount(summary.frameCount)} frames · ${formatCount(summary.checkpointCount)} checkpoints · ${formatBytes(summary.bytes)} · ${summary.format} · ${map}`;
  els.sigCount.textContent = String(summary.signals.length);
  renderSignals();
  renderMarks();
  renderEvents();
  renderTransport();
}

function renderTransport(): void {
  els.timeReadout.textContent = formatUs(state.playhead);
  els.spanReadout.textContent = `span ${formatSpan(state.span)}`;
  els.play.textContent = state.playing ? "❚❚" : "▶";
  els.rate.textContent = `${state.rate}×`;
  const frame = state.frame;
  if (!frame) {
    els.frameReadout.textContent = "";
  } else if (frame.messageId != null) {
    els.frameReadout.textContent = `f ${formatCount(frame.ordinal)} · ${hexId(frame.messageId)} ${frame.messageName}`;
  } else {
    els.frameReadout.textContent = `f ${formatCount(frame.ordinal)} · ${frame.messageName}`;
  }
}

function visibleSignals() {
  const signals = state.summary?.signals ?? [];
  const q = state.filter.trim().toLowerCase();
  if (!q) return signals;
  return signals.filter((signal) => {
    const hay = `${signal.name} ${signal.unit} ${signal.messageName}`.toLowerCase();
    return hay.includes(q);
  });
}

function renderSignals(): void {
  els.sigList.replaceChildren();
  const signals = visibleSignals();
  if (!state.summary) return;
  if (signals.length === 0) {
    const note = document.createElement("p");
    note.className = "empty-note";
    note.textContent = state.summary.signals.length === 0 ? "No decoded signals" : "No signals match";
    els.sigList.append(note);
    return;
  }
  for (const signal of signals) {
    const row = document.createElement("label");
    row.className = "sig";
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = state.plotted.includes(signal.name);
    input.addEventListener("change", () => {
      if (input.checked) state.plotted = [...state.plotted, signal.name];
      else state.plotted = state.plotted.filter((name) => name !== signal.name);
      state.dirty = true;
      renderChrome();
      draw();
      void refresh();
      void refreshOverview();
    });
    const swatch = document.createElement("span");
    swatch.className = "swatch";
    swatch.style.background = colorFor(signal.name);
    const copy = document.createElement("span");
    copy.className = "sig-copy";
    const name = document.createElement("span");
    name.className = "sig-name";
    name.textContent = signal.name;
    const meta = document.createElement("span");
    meta.className = "sig-meta";
    const id = signal.messageId == null ? signal.messageName : `${hexId(signal.messageId)} ${signal.messageName}`;
    meta.textContent = signal.unit ? `${signal.unit} · ${id}` : id;
    copy.append(name, meta);
    row.append(input, swatch, copy);
    els.sigList.append(row);
  }
}

function renderMarks(): void {
  els.markList.replaceChildren();
  if (state.bookmarks.length === 0) {
    const note = document.createElement("p");
    note.className = "empty-note";
    note.textContent = "B drops a mark at the playhead";
    els.markList.append(note);
    return;
  }
  for (const mark of state.bookmarks) {
    const row = document.createElement("div");
    row.className = "mark-row-wrap";
    const button = document.createElement("button");
    button.type = "button";
    button.className = `mark-row${state.selectedMark === mark.id ? " is-selected" : ""}`;
    const dot = document.createElement("span");
    dot.className = "mark-dot";
    const time = document.createElement("span");
    time.className = "mark-t";
    time.textContent = formatUs(mark.tUs);
    const label = document.createElement("span");
    label.className = "mark-l";
    label.textContent = mark.label;
    button.append(dot, time, label);
    button.addEventListener("click", () => {
      state.selectedMark = mark.id;
      scrubTo(mark.tUs);
      renderMarks();
    });
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "mark-x";
    remove.textContent = "×";
    remove.setAttribute("aria-label", `Remove ${mark.label}`);
    remove.addEventListener("click", () => removeBookmark(mark.id));
    const line = document.createElement("div");
    line.style.display = "flex";
    line.append(button, remove);
    button.style.flex = "1";
    row.append(line);
    els.markList.append(row);
  }
}

function renderEvents(): void {
  els.eventList.replaceChildren();
  const events = state.summary?.events ?? [];
  if (events.length === 0) {
    const note = document.createElement("p");
    note.className = "empty-note";
    note.textContent = "This log has no event marks";
    els.eventList.append(note);
    return;
  }
  for (const event of events) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "event-row";
    const dot = document.createElement("span");
    dot.className = "event-dot";
    const time = document.createElement("span");
    time.className = "event-t";
    time.textContent = formatUs(event.tUs);
    const label = document.createElement("span");
    label.className = "event-l";
    label.textContent = event.label;
    button.append(dot, time, label);
    button.addEventListener("click", () => scrubTo(event.tUs));
    els.eventList.append(button);
  }
  if (state.summary?.eventsTruncated) {
    const note = document.createElement("p");
    note.className = "empty-note";
    note.textContent = "Event list truncated";
    els.eventList.append(note);
  }
}

function renderLegend(): void {
  els.legend.replaceChildren();
  for (const name of state.plotted) {
    const signal = state.summary?.signals.find((item) => item.name === name);
    const value = state.frame?.values.find((item) => item.name === name);
    const item = document.createElement("div");
    item.className = "legend-item";
    const swatch = document.createElement("i");
    swatch.style.background = colorFor(name);
    const label = document.createElement("span");
    label.className = "name";
    label.textContent = name;
    const reading = document.createElement("span");
    reading.className = "val";
    reading.textContent = value ? formatValue(value.value) : "—";
    const unit = document.createElement("span");
    unit.className = "unit";
    unit.textContent = signal?.unit ?? "";
    item.append(swatch, label, reading, unit);
    els.legend.append(item);
  }
  els.scaleNote.hidden = state.plotted.length < 2;
}

function stageMessage(): string | null {
  if (!state.summary) return "No log on the deck. Load the cluster sample or open a CAN log.";
  if (state.summary.signals.length === 0) return "No signals yet. Open a JSON signal map to decode frames.";
  if (state.plotted.length === 0) return "Select signals to overlay them on the scope.";
  return null;
}

function draw(): void {
  const summary = state.summary;
  const message = stageMessage();
  els.stageMsg.hidden = message == null;
  els.stageMsg.textContent = message ?? "";
  const traces: Trace[] = state.plotted.map((name) => {
    const series = state.series.find((item) => item.name === name);
    const points = series?.points ?? [];
    const info = summary?.signals.find((signal) => signal.name === name);
    let min = info?.min ?? null;
    let max = info?.max ?? null;
    if (min == null || max == null || max <= min) {
      min = points.reduce((lo, point) => Math.min(lo, point.v), Infinity);
      max = points.reduce((hi, point) => Math.max(hi, point.v), -Infinity);
      if (!Number.isFinite(min) || !Number.isFinite(max)) {
        min = 0;
        max = 1;
      }
    }
    return { name, color: colorFor(name), points, min, max };
  });
  drawPlot(els.plot, state.view, state.playhead, traces);
  const domain = summary
    ? { t0: summary.tStartUs, t1: summary.tEndUs }
    : { t0: 0, t1: 1 };
  const view = summary ? windowFor(state.playhead, state.span, summary) : state.view;
  const marks: TimelineMark[] = [
    ...(summary?.events.map((event) => ({ t: event.tUs, kind: "event" as const })) ?? []),
    ...state.bookmarks.map((mark) => ({ t: mark.tUs, kind: "mark" as const })),
  ];
  drawTimeline(els.timeline, domain, view, state.playhead, marks, state.overview);
  renderTransport();
  renderLegend();
}

function scrubTo(t: number): void {
  if (!state.summary) return;
  state.playhead = clamp(t, state.summary.tStartUs, state.summary.tEndUs);
  state.dirty = true;
  draw();
  scheduleRefresh();
}

function scheduleRefresh(): void {
  window.clearTimeout(refreshTimer);
  refreshTimer = window.setTimeout(() => void refresh(), 40);
}

async function refresh(): Promise<void> {
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
  const token = ++queryToken;
  const view = windowFor(state.playhead, state.span, summary);
  const width = els.plot.getBoundingClientRect().width || 800;
  const maxPoints = Math.max(200, Math.min(4000, Math.round(width * 2)));
  try {
    const [series, frame] = await Promise.all([
      state.plotted.length
        ? api.query({ t0Us: view.t0, t1Us: view.t1, signals: state.plotted, maxPoints })
        : Promise.resolve([] as Series[]),
      api.frameAt(state.playhead),
    ]);
    if (token !== queryToken) return;
    state.series = series;
    state.frame = frame;
    state.view = view;
    clearError();
    draw();
  } catch (err) {
    if (token === queryToken) setError(errText(err));
  } finally {
    queryFlight = false;
    if (queryAgain) {
      queryAgain = false;
      void refresh();
    }
  }
}

async function refreshOverview(): Promise<void> {
  const summary = state.summary;
  const name = state.plotted[0];
  if (!summary || !name) {
    state.overview = null;
    state.overviewName = null;
    draw();
    return;
  }
  if (state.overviewName === name && state.overview) return;
  try {
    const series = await api.query({
      t0Us: summary.tStartUs,
      t1Us: summary.tEndUs,
      signals: [name],
      maxPoints: 700,
    });
    state.overview = series[0]?.points ?? null;
    state.overviewName = name;
    draw();
  } catch {
    state.overview = null;
  }
}

function adoptSummary(summary: Summary, mode: "fresh" | "keep"): void {
  state.summary = summary;
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
  } else {
    state.plotted = state.plotted.filter((name) => summary.signals.some((signal) => signal.name === name));
    if (state.plotted.length === 0) state.plotted = defaultPlotted(summary);
    state.playhead = clamp(state.playhead, summary.tStartUs, summary.tEndUs);
    state.span = clamp(state.span, MIN_SPAN, duration(summary));
    state.overview = null;
    state.overviewName = null;
  }
  clearError();
  renderChrome();
  void refresh();
  void refreshOverview();
}

function applyProject(opened: ProjectOpen, path: string | null): void {
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
  if (opened.warnings.length) setError(opened.warnings.join(" "));
  else clearError();
  renderChrome();
  void refresh();
  void refreshOverview();
}

function currentProject(): ProjectFile {
  return {
    format: "signal-loom",
    version: 1,
    logPath: state.summary?.logPath ?? state.summary?.logLabel ?? "",
    signalMapPath: state.summary?.mapPath ?? null,
    bookmarks: state.bookmarks,
    view: {
      playheadUs: Math.round(state.playhead),
      spanUs: Math.round(state.span),
      plotted: [...state.plotted],
    },
  };
}

async function loadSample(): Promise<void> {
  await withBusy("Indexing cluster sample", async () => {
    adoptSummary(await api.openSample(), "fresh");
  });
}

async function openLog(): Promise<void> {
  if (!api.inTauri()) {
    els.fileLog.click();
    return;
  }
  const path = await pick([{ name: "Logs", extensions: ["slog", "slbin", "csv", "txt", "log"] }]);
  if (!path) return;
  await withBusy(`Indexing ${basename(path)}`, async () => {
    adoptSummary(await api.openPath(path), "fresh");
  });
}

async function openMap(): Promise<void> {
  if (!api.inTauri()) {
    els.fileMap.click();
    return;
  }
  const path = await pick([{ name: "Signal map", extensions: ["json"] }]);
  if (!path) return;
  await withBusy(`Decoding with ${basename(path)}`, async () => {
    adoptSummary(await api.openMapPath(path), "keep");
    state.dirty = true;
    renderChrome();
  });
}

async function openProject(): Promise<void> {
  if (!api.inTauri()) {
    els.fileProject.click();
    return;
  }
  const path = await pick([{ name: "Signal Loom project", extensions: ["loom"] }]);
  if (!path) return;
  await withBusy(`Opening ${basename(path)}`, async () => {
    applyProject(await api.openProjectPath(path), path);
  });
}

async function saveProject(asNew: boolean): Promise<void> {
  const project = currentProject();
  if (!state.summary) {
    setError("Nothing to save yet.");
    return;
  }
  if (!api.inTauri()) {
    download(`${basename(state.projectPath ?? "session.loom")}`, JSON.stringify(project, null, 2));
    state.dirty = false;
    renderChrome();
    return;
  }
  let path = asNew ? null : state.projectPath;
  if (!path) path = await pickSave();
  if (!path) return;
  if (!path.toLowerCase().endsWith(".loom")) path += ".loom";
  await withBusy("Saving project", async () => {
    await api.writeProject(path!, project);
    state.projectPath = path;
    state.dirty = false;
    renderChrome();
  });
}

function download(name: string, text: string): void {
  const blob = new Blob([text], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  URL.revokeObjectURL(url);
}

async function pick(filters: { name: string; extensions: string[] }[]): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ multiple: false, filters });
  if (typeof picked === "string") return picked;
  if (Array.isArray(picked)) return picked[0] ?? null;
  return null;
}

async function pickSave(): Promise<string | null> {
  const { save } = await import("@tauri-apps/plugin-dialog");
  return save({
    defaultPath: state.projectPath ?? "session.loom",
    filters: [{ name: "Signal Loom project", extensions: ["loom"] }],
  });
}

async function stepFrame(direction: "next" | "prev"): Promise<void> {
  if (!state.summary) return;
  try {
    const frame = await api.step(state.playhead, direction);
    if (!frame) {
      setError(direction === "next" ? "End of log." : "Start of log.");
      return;
    }
    clearError();
    state.playhead = frame.tUs;
    state.frame = frame;
    state.dirty = true;
    draw();
    scheduleRefresh();
  } catch (err) {
    setError(errText(err));
  }
}

function markers(): { t: number }[] {
  const events = state.summary?.events.map((event) => ({ t: event.tUs })) ?? [];
  const marks = state.bookmarks.map((mark) => ({ t: mark.tUs }));
  return [...events, ...marks].sort((a, b) => a.t - b.t);
}

function stepEvent(direction: 1 | -1): void {
  const list = markers();
  if (list.length === 0) {
    setError("No events or bookmarks in this log.");
    return;
  }
  const hit =
    direction > 0
      ? list.find((item) => item.t > state.playhead + 0.5)
      : [...list].reverse().find((item) => item.t < state.playhead - 0.5);
  if (!hit) {
    setError(direction > 0 ? "No later event." : "No earlier event.");
    return;
  }
  clearError();
  scrubTo(hit.t);
}

function addBookmark(label?: string): void {
  if (!state.summary) return;
  const text = (label ?? els.markLabel.value).trim() || `Mark ${state.bookmarks.length + 1}`;
  const mark: Bookmark = {
    id: `m-${Date.now().toString(36)}-${state.bookmarks.length + 1}`,
    tUs: Math.round(state.playhead),
    label: text,
  };
  state.bookmarks = [...state.bookmarks, mark].sort((a, b) => a.tUs - b.tUs);
  state.selectedMark = mark.id;
  state.dirty = true;
  els.markLabel.value = "";
  renderMarks();
  draw();
}

function removeBookmark(id: string): void {
  state.bookmarks = state.bookmarks.filter((mark) => mark.id !== id);
  if (state.selectedMark === id) state.selectedMark = null;
  state.dirty = true;
  renderMarks();
  draw();
}

function zoom(factor: number): void {
  if (!state.summary) return;
  state.span = clamp(state.span * factor, MIN_SPAN, duration(state.summary));
  state.dirty = true;
  draw();
  scheduleRefresh();
}

function togglePlay(): void {
  if (!state.summary) return;
  state.playing = !state.playing;
  if (state.playing && state.playhead >= state.summary.tEndUs) state.playhead = state.summary.tStartUs;
  renderTransport();
  if (state.playing) {
    playStamp = 0;
    requestAnimationFrame(tick);
  }
}

function tick(now: number): void {
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
  if (state.playing) requestAnimationFrame(tick);
}

function typingTarget(target: EventTarget | null): boolean {
  return target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement;
}

function onKey(event: KeyboardEvent): void {
  const mod = event.metaKey || event.ctrlKey;
  if (event.key === "Escape") {
    els.help.close();
    els.sigFilter.blur();
    return;
  }
  if (mod && event.key.toLowerCase() === "o" && !event.shiftKey) {
    event.preventDefault();
    void openLog();
    return;
  }
  if (mod && event.key.toLowerCase() === "o" && event.shiftKey) {
    event.preventDefault();
    void openProject();
    return;
  }
  if (mod && event.key.toLowerCase() === "s") {
    event.preventDefault();
    void saveProject(event.shiftKey);
    return;
  }
  if (mod && event.shiftKey && event.key.toLowerCase() === "l") {
    event.preventDefault();
    void loadSample();
    return;
  }
  if (typingTarget(event.target)) return;
  if (event.key === "?" || (event.shiftKey && event.key === "/")) {
    event.preventDefault();
    if (!els.help.open) els.help.showModal();
    return;
  }
  if (event.key === "/") {
    event.preventDefault();
    els.sigFilter.focus();
    return;
  }
  if (event.key === " ") {
    event.preventDefault();
    togglePlay();
    return;
  }
  if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
    event.preventDefault();
    const dir = event.key === "ArrowRight" ? 1 : -1;
    if (event.shiftKey) stepEvent(dir as 1 | -1);
    else void stepFrame(dir > 0 ? "next" : "prev");
    return;
  }
  if (event.key === "b" || event.key === "B") {
    event.preventDefault();
    addBookmark();
    return;
  }
  if ((event.key === "Delete" || event.key === "Backspace") && state.selectedMark) {
    event.preventDefault();
    removeBookmark(state.selectedMark);
    return;
  }
  if (event.key === "[" || event.key === "]") {
    event.preventDefault();
    zoom(event.key === "]" ? 0.7 : 1.4);
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

function onWheel(event: WheelEvent): void {
  if (!state.summary) return;
  event.preventDefault();
  if (event.shiftKey) {
    scrubTo(state.playhead + (event.deltaY / 400) * state.span);
  } else {
    const factor = Math.exp(event.deltaY * 0.0012);
    zoom(factor);
  }
}

async function ingestFile(file: File): Promise<void> {
  const name = file.name.toLowerCase();
  await withBusy(`Indexing ${file.name}`, async () => {
    if (name.endsWith(".loom")) {
      applyProject(await api.openProjectJson(await file.text()), null);
      return;
    }
    if (name.endsWith(".json")) {
      const text = await file.text();
      if (text.includes('"format"') && text.includes("signal-loom")) {
        applyProject(await api.openProjectJson(text), null);
      } else {
        adoptSummary(await api.openMapJson(text), "keep");
        state.dirty = true;
        renderChrome();
      }
      return;
    }
    adoptSummary(await api.openBytes(file.name, await file.arrayBuffer()), "fresh");
  });
}

async function ingestPath(path: string): Promise<void> {
  const lower = path.toLowerCase();
  await withBusy(`Indexing ${basename(path)}`, async () => {
    if (lower.endsWith(".loom")) {
      applyProject(await api.openProjectPath(path), path);
      return;
    }
    if (lower.endsWith(".json")) {
      adoptSummary(await api.openMapPath(path), "keep");
      state.dirty = true;
      renderChrome();
      return;
    }
    adoptSummary(await api.openPath(path), "fresh");
  });
}

function bind(): void {
  els.runtime.textContent = api.inTauri() ? "Desktop" : "Browser preview";
  document.getElementById("btn-sample")?.addEventListener("click", () => void loadSample());
  document.getElementById("btn-open")?.addEventListener("click", () => void openLog());
  document.getElementById("btn-map")?.addEventListener("click", () => void openMap());
  document.getElementById("btn-project")?.addEventListener("click", () => void openProject());
  els.save.addEventListener("click", () => void saveProject(false));
  document.getElementById("btn-help")?.addEventListener("click", () => {
    if (!els.help.open) els.help.showModal();
  });
  document.getElementById("prev-event")?.addEventListener("click", () => stepEvent(-1));
  document.getElementById("next-event")?.addEventListener("click", () => stepEvent(1));
  document.getElementById("prev-frame")?.addEventListener("click", () => void stepFrame("prev"));
  document.getElementById("next-frame")?.addEventListener("click", () => void stepFrame("next"));
  els.play.addEventListener("click", togglePlay);
  els.rate.addEventListener("click", () => {
    const index = RATES.indexOf(state.rate);
    state.rate = RATES[(index + 1) % RATES.length];
    renderTransport();
  });
  document.getElementById("zoom-in")?.addEventListener("click", () => zoom(0.7));
  document.getElementById("zoom-out")?.addEventListener("click", () => zoom(1.4));
  document.getElementById("zoom-all")?.addEventListener("click", () => {
    if (!state.summary) return;
    state.span = duration(state.summary);
    state.dirty = true;
    draw();
    scheduleRefresh();
  });
  els.markForm.addEventListener("submit", (event) => {
    event.preventDefault();
    addBookmark();
  });
  els.sigFilter.addEventListener("input", () => {
    state.filter = els.sigFilter.value;
    renderSignals();
  });
  els.fileLog.addEventListener("change", () => {
    const file = els.fileLog.files?.[0];
    els.fileLog.value = "";
    if (file) void ingestFile(file);
  });
  els.fileMap.addEventListener("change", () => {
    const file = els.fileMap.files?.[0];
    els.fileMap.value = "";
    if (file) void ingestFile(file);
  });
  els.fileProject.addEventListener("change", () => {
    const file = els.fileProject.files?.[0];
    els.fileProject.value = "";
    if (file) void ingestFile(file);
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
    const rect = els.plot.getBoundingClientRect();
    const view = windowFor(state.playhead, state.span, state.summary);
    const u = (event.clientX - rect.left) / Math.max(1, rect.width);
    scrubTo(view.t0 + clamp(u, 0, 1) * (view.t1 - view.t0));
  });
  els.plot.addEventListener("wheel", onWheel, { passive: false });
  els.timeline.addEventListener("wheel", onWheel, { passive: false });
  window.addEventListener("keydown", onKey);
  window.addEventListener("resize", () => draw());
  new ResizeObserver(() => draw()).observe(els.viewport);

  let dragDepth = 0;
  window.addEventListener("dragenter", (event) => {
    event.preventDefault();
    dragDepth += 1;
    els.drop.hidden = false;
  });
  window.addEventListener("dragover", (event) => {
    event.preventDefault();
  });
  window.addEventListener("dragleave", () => {
    dragDepth = Math.max(0, dragDepth - 1);
    if (dragDepth === 0) els.drop.hidden = true;
  });
  window.addEventListener("drop", (event) => {
    event.preventDefault();
    dragDepth = 0;
    els.drop.hidden = true;
    const file = event.dataTransfer?.files?.[0];
    if (file) void ingestFile(file);
  });

  if (api.inTauri()) {
    void import("@tauri-apps/api/webview").then(({ getCurrentWebview }) => {
      void getCurrentWebview().onDragDropEvent((event) => {
        if (event.payload.type === "enter" || event.payload.type === "over") els.drop.hidden = false;
        if (event.payload.type === "leave") els.drop.hidden = true;
        if (event.payload.type === "drop") {
          els.drop.hidden = true;
          const path = event.payload.paths[0];
          if (path) void ingestPath(path);
        }
      });
    });
  }
}

async function waitForEngine(): Promise<void> {
  for (let attempt = 0; attempt < 180; attempt += 1) {
    if (await api.health()) return;
    if (attempt === 2) paintBusy("Compiling the local indexer…");
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(
    "Local engine is not running. Use npm run dev:preview, or open the desktop shell with npm run tauri dev.",
  );
}

async function boot(): Promise<void> {
  bind();
  draw();
  if (api.inTauri()) {
    await loadSample();
    return;
  }
  await withBusy("Waiting for the local engine", async () => {
    await waitForEngine();
    paintBusy("Indexing cluster sample");
    adoptSummary(await api.openSample(), "fresh");
  });
}

void boot();
