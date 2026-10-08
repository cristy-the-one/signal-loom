import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import * as api from "./api";
import { familyColor, severityOf } from "./family";
import { drawBus, drawGauges, readingsFrom, slotAt, slotSpec, type ClusterBindings, type SlotId } from "./gauges";
import { drawPlot, formatHover, heldValue, timeOnPlot, type Trace } from "./plot";
import { drawTimeline, timeAt, type TimelineMark } from "./timeline";
import {
  basename,
  clamp,
  errText,
  formatBytes,
  formatCount,
  formatReading,
  formatSpan,
  formatUs,
  formatValue,
  hexId,
  readoutFor,
} from "./format";
import type {
  Bookmark,
  BusLoad,
  FrameHit,
  MathChannel,
  Note,
  Point,
  ProjectFile,
  ProjectOpen,
  Series,
  SignalInfo,
  Summary,
  ThresholdTrigger,
  ValueRead,
} from "./types";
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
  veilDetail: must<HTMLElement>("veil-detail"),
  veilFill: must<HTMLElement>("veil-fill"),
  veilCancel: must<HTMLButtonElement>("veil-cancel"),
  warn: must<HTMLElement>("warn"),
  dbcChannel: must<HTMLInputElement>("dbc-channel"),
  drop: must<HTMLElement>("drop"),
  help: must<HTMLDialogElement>("help"),
  save: must<HTMLButtonElement>("btn-save"),
  fileLog: must<HTMLInputElement>("file-log"),
  fileMap: must<HTMLInputElement>("file-map"),
  fileProject: must<HTMLInputElement>("file-project"),
  fileCompare: must<HTMLInputElement>("file-compare"),
  gauges: must<HTMLCanvasElement>("gauges"),
  bus: must<HTMLCanvasElement>("bus"),
  crosshair: must<HTMLElement>("crosshair"),
  crossTime: must<HTMLElement>("cross-time"),
  crossVals: must<HTMLElement>("cross-vals"),
  cursorRead: must<HTMLElement>("cursor-read"),
  noteBody: must<HTMLInputElement>("note-body"),
  noteList: must<HTMLElement>("note-list"),
  mathList: must<HTMLElement>("math-list"),
  trigList: must<HTMLElement>("trig-list"),
  compareLabel: must<HTMLElement>("compare-label"),
  compareOffset: must<HTMLInputElement>("compare-offset"),
  captureArm: must<HTMLInputElement>("capture-arm"),
  captureIface: must<HTMLInputElement>("capture-iface"),
  captureMs: must<HTMLInputElement>("capture-ms"),
  captureBtn: must<HTMLButtonElement>("btn-capture"),
};

interface View {
  t0: number;
  t1: number;
}

type RailTab = "marks" | "notes" | "math" | "alerts" | "drive";

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
  held: ValueRead[];
  bus: BusLoad | null;
  hoverT: number | null;
  hoverX: number;
  cursorA: number | null;
  cursorB: number | null;
  cursorText: string;
  math: MathChannel[];
  triggers: ThresholdTrigger[];
  notes: Note[];
  compareOn: boolean;
  comparePath: string | null;
  compareOffsetUs: number;
  /** Signals the user put in the cluster's gauges and lamps. */
  cluster: ClusterBindings;
  tab: RailTab;
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
  held: [],
  bus: null,
  hoverT: null,
  hoverX: 0,
  cursorA: null,
  cursorB: null,
  cursorText: "",
  math: [],
  triggers: [],
  notes: [],
  compareOn: false,
  comparePath: null,
  compareOffsetUs: 0,
  cluster: {},
  tab: "marks",
};

/** Bumped when the data a plot response belongs to changes: a new summary or a new plotted set. */
let dataGen = 0;
let cursorSeq = 0;
let deckQueue: Promise<void> = Promise.resolve();
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

/** Changes the plotted set; plot responses and cursor stats asked for the old set are dropped. */
function setPlotted(names: string[]): void {
  state.plotted = names;
  dataGen += 1;
  void refreshCursors();
}

function duration(summary: Summary): number {
  return Math.max(1, summary.tEndUs - summary.tStartUs);
}

/** A signal has data when the log decoded at least one sample of it. */
function hasData(signal: SignalInfo): boolean {
  return signal.min != null || signal.messageName === "Math";
}

/** Three signals worth a first look: ones with data, not checksums or counters. */
function defaultPlotted(summary: Summary): string[] {
  const live = summary.signals.filter(hasData);
  const names = new Set(live.map((signal) => signal.name));
  const preferred = ["VehicleSpeed", "EngineRPM", "BrakePressure"].filter((name) => names.has(name));
  if (preferred.length) return preferred;
  const housekeeping = /checksum|counter|crc|alive/i;
  const useful = live.filter((signal) => !housekeeping.test(signal.name));
  const pool = useful.length ? useful : live;
  // A signal that never changes draws a flat line: prefer ones that move.
  const moves = (signal: SignalInfo) => signal.min != null && signal.max != null && signal.max > signal.min;
  return [...pool.filter(moves), ...pool.filter((signal) => !moves(signal))]
    .slice(0, 3)
    .map((signal) => signal.name);
}

function colorFor(name: string): string {
  const base = name.replace(/ · B$/, "");
  const index = state.summary?.signals.findIndex((signal) => signal.name === base) ?? 0;
  const signal = state.summary?.signals.find((item) => item.name === base);
  return familyColor(base, signal?.messageName ?? "", index < 0 ? 0 : index);
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

/** Who raised a banner message; only the owner's later success clears it. */
type ErrorOwner = "query" | "project" | "action";

const errors = new Map<ErrorOwner, string>();

function paintErrors(): void {
  const text = [...errors.values()].join(" ");
  els.error.hidden = text === "";
  els.error.textContent = text;
}

function setError(message: string, owner: ErrorOwner): void {
  errors.set(owner, message);
  paintErrors();
}

function clearError(owner?: ErrorOwner): void {
  if (owner) errors.delete(owner);
  else errors.clear();
  paintErrors();
}

let noticeTimer: ReturnType<typeof setTimeout> | undefined;

/** A short message that is not an error, such as a saved export or the end of the log. It clears itself. */
function setNotice(message: string): void {
  const notice = document.getElementById("notice") as HTMLElement;
  notice.textContent = message;
  notice.hidden = false;
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => {
    notice.hidden = true;
    notice.textContent = "";
  }, 6000);
}

function paintBusy(label?: string): void {
  const active = state.busy > 0;
  els.viewport.classList.toggle("is-busy", active);
  els.veil.hidden = !active;
  if (label) els.veilLabel.textContent = label;
  if (!active) {
    els.veilDetail.textContent = "";
    els.veilFill.style.width = "0";
    els.veilCancel.hidden = true;
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

async function withIndex(
  label: string,
  start: () => Promise<void>,
): Promise<Summary> {
  state.busy += 1;
  paintBusy(label);
  els.veilCancel.hidden = false;
  let stop = false;
  const onCancel = () => {
    stop = true;
    void api.cancelIndex();
    els.veilLabel.textContent = "Cancelling";
  };
  els.veilCancel.addEventListener("click", onCancel);
  try {
    await start();
    for (;;) {
      const tick = await api.indexProgress();
      if (tick.error) throw new Error(tick.error);
      if (tick.done && tick.summary) return tick.summary;
      if (tick.idle && !tick.done) throw new Error("indexing did not start");
      const pct = tick.bytesTotal
        ? Math.min(99, Math.round((100 * tick.bytesDone) / tick.bytesTotal))
        : 0;
      els.veilFill.style.width = `${pct}%`;
      els.veilLabel.textContent = stop ? "Cancelling" : `${label} · ${pct}%`;
      const skipped = tick.skipped ? ` · ${formatCount(tick.skipped)} skipped` : "";
      els.veilDetail.textContent = `${formatCount(tick.frames)} frames${skipped}`;
      await sleep(80);
    }
  } finally {
    els.veilCancel.removeEventListener("click", onCancel);
    state.busy = Math.max(0, state.busy - 1);
    paintBusy();
  }
}

function mapChannel(): number {
  const value = Number(els.dbcChannel.value);
  if (!Number.isFinite(value) || value < 0) return 0;
  return Math.min(255, Math.round(value));
}

/** Runs deck mutations one at a time, each building its new list from the state the last one left. */
function serialDeck(work: () => Promise<void>): Promise<void> {
  const run = deckQueue.then(work);
  deckQueue = run.catch(() => undefined);
  return run;
}

async function withBusy(label: string, work: () => Promise<void>): Promise<void> {
  state.busy += 1;
  paintBusy(label);
  await new Promise((resolve) => requestAnimationFrame(() => resolve(undefined)));
  try {
    await work();
  } catch (err) {
    setError(errText(err), "action");
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
  const fit = summary.mapMatch ? ` (fits ${summary.mapMatch.matched} of ${summary.mapMatch.total} messages)` : "";
  const map = summary.mapLabel ? `${summary.mapLabel}${fit}` : "no signal map";
  const skipped = summary.skippedRecords ? ` · ${formatCount(summary.skippedRecords)} skipped` : "";
  els.logMeta.textContent = `${formatCount(summary.frameCount)} frames · ${formatCount(summary.checkpointCount)} checkpoints · ${formatBytes(summary.bytes)} · ${summary.format}${skipped} · ${map}`;
  const warnings = summary.warnings ?? [];
  if (warnings.length || summary.skippedRecords) {
    const head = summary.skippedRecords
      ? `${formatCount(summary.skippedRecords)} records skipped`
      : "Import notes";
    const tail = warnings.slice(0, 3).join(" · ");
    els.warn.hidden = false;
    els.warn.textContent = tail ? `${head}. ${tail}` : head;
  } else {
    els.warn.hidden = true;
    els.warn.textContent = "";
  }
  els.sigCount.textContent = String(summary.signals.length);
  renderSignals();
  renderMarks();
  renderEvents();
  renderNotes();
  renderMath();
  renderTriggers();
  renderCompare();
  renderTransport();
}

function renderTransport(): void {
  els.timeReadout.textContent = formatUs(state.playhead);
  els.spanReadout.textContent = `span ${formatSpan(state.span)}`;
  els.play.textContent = state.playing ? "❚❚" : "▶";
  els.rate.textContent = `${state.rate}×`;
  els.cursorRead.textContent = state.cursorText;
  const frame = state.frame;
  if (!frame) {
    els.frameReadout.textContent = "";
  } else if (frame.messageId != null) {
    const id = frame.extended ? `${hexId(frame.messageId)}x` : hexId(frame.messageId);
    els.frameReadout.textContent = `f ${formatCount(frame.ordinal)} · ${id} ${frame.messageName}`;
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
  // Signals this log carries first; the rest of the map is listed, dimmed.
  const ordered = [...signals.filter(hasData), ...signals.filter((signal) => !hasData(signal))];
  for (const signal of ordered) {
    const row = document.createElement("label");
    row.className = hasData(signal) ? "sig" : "sig is-empty";
    if (!hasData(signal)) row.title = "No frame in this log carries this signal";
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = state.plotted.includes(signal.name);
    input.addEventListener("change", () => {
      if (input.checked) setPlotted([...state.plotted, signal.name]);
      else setPlotted(state.plotted.filter((name) => name !== signal.name));
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

let eventRows: { tUs: number; button: HTMLElement; selected: boolean }[] = [];

/** Marks the event rows at the playhead; cheap enough to run on every draw. */
function syncEventSelection(): void {
  for (const row of eventRows) {
    const selected = Math.abs(row.tUs - state.playhead) <= 500;
    if (selected === row.selected) continue;
    row.selected = selected;
    row.button.classList.toggle("is-selected", selected);
    if (selected) row.button.scrollIntoView({ block: "nearest" });
  }
}

function renderEvents(): void {
  els.eventList.replaceChildren();
  eventRows = [];
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
    const severity = severityOf(event.label);
    button.classList.add(severity);
    eventRows.push({ tUs: event.tUs, button, selected: false });
    const dot = document.createElement("span");
    dot.className = `event-dot ${severity}`;
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
  syncEventSelection();
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
    const held = state.held.find((item) => item.name === name);
    const item = document.createElement("div");
    item.className = "legend-item";
    const swatch = document.createElement("i");
    swatch.style.background = colorFor(name);
    const label = document.createElement("span");
    label.className = "name";
    label.textContent = name;
    const reading = document.createElement("span");
    reading.className = "val";
    const readout = readoutFor(signal);
    reading.textContent = held
      ? held.label
        ? `${formatReading(held.value, readout)} ${held.label}`
        : formatReading(held.value, readout)
      : "—".padStart(readout.width);
    const unit = document.createElement("span");
    unit.className = "unit";
    unit.textContent = signal?.unit ?? "";
    item.append(swatch, label, reading, unit);
    els.legend.append(item);
  }
  els.scaleNote.hidden = state.plotted.length < 2;
}

function stageMessage(): string | null {
  if (!state.summary) return "No log on the deck. Load the synthetic hypercar sample or open a CAN log.";
  if (state.summary.signals.length === 0) return "No signals yet. Open a JSON signal map to decode frames.";
  if (state.plotted.length === 0) return "Select signals to overlay them on the scope.";
  return null;
}

function draw(): void {
  const summary = state.summary;
  const message = stageMessage();
  els.stageMsg.hidden = message == null;
  els.stageMsg.textContent = message ?? "";
  const names = traceNames();
  const traces: Trace[] = names.map((name) => {
    const base = name.replace(/ · B$/, "");
    const series = state.series.find((item) => item.name === name);
    const points = series?.points ?? [];
    const info = summary?.signals.find((signal) => signal.name === base);
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
    return { name, color: colorFor(name), points, min, max, dashed: name.endsWith(" · B") };
  });
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

function traceNames(): string[] {
  const names = [...state.plotted];
  if (!state.compareOn) return names;
  for (const name of state.plotted) {
    if (state.series.some((series) => series.name === `${name} · B`)) names.push(`${name} · B`);
  }
  return names;
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
    const signal = state.summary?.signals.find((item) => item.name === trace.name.replace(/ · B$/, ""));
    const value = heldValue(trace.points, state.hoverT ?? 0);
    return formatHover(trace.name.padEnd(nameWidth), value, signal?.unit ?? "", readoutFor(signal));
  });
  els.crossVals.textContent = lines.join("\n");
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
  const gen = dataGen;
  const view = windowFor(state.playhead, state.span, summary);
  const width = els.plot.getBoundingClientRect().width || 800;
  const maxPoints = Math.max(200, Math.min(4000, Math.round(width * 2)));
  try {
    const bus = oneSecond(state.playhead, summary);
    const [series, frame, held, load] = await Promise.all([
      state.plotted.length
        ? api.query({
            t0Us: view.t0,
            t1Us: view.t1,
            signals: state.plotted,
            maxPoints,
            includeCompare: state.compareOn,
          })
        : Promise.resolve([] as Series[]),
      api.frameAt(state.playhead),
      api.valuesAt(state.playhead),
      api.busLoad(bus.t0, bus.t1),
    ]);
    if (gen !== dataGen) return;
    state.series = series;
    state.frame = frame;
    state.held = held;
    state.bus = load;
    state.view = view;
    clearError("query");
    draw();
  } catch (err) {
    if (gen === dataGen) setError(errText(err), "query");
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
  const gen = dataGen;
  try {
    const series = await api.query({
      t0Us: summary.tStartUs,
      t1Us: summary.tEndUs,
      signals: [name],
      maxPoints: 700,
    });
    if (gen !== dataGen) return;
    state.overview = series[0]?.points ?? null;
    state.overviewName = name;
    draw();
  } catch {
    if (gen === dataGen) state.overview = null;
  }
}

function oneSecond(playhead: number, summary: Summary): View {
  const half = 500_000;
  let t0 = playhead - half;
  let t1 = playhead + half;
  if (t0 < summary.tStartUs) {
    t0 = summary.tStartUs;
    t1 = Math.min(summary.tEndUs, t0 + 1_000_000);
  }
  if (t1 > summary.tEndUs) {
    t1 = summary.tEndUs;
    t0 = Math.max(summary.tStartUs, t1 - 1_000_000);
  }
  return { t0, t1 };
}

function resetDeck(): void {
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

function adoptSummary(summary: Summary, mode: "fresh" | "keep"): void {
  dataGen += 1;
  state.summary = summary;
  timeoutInput().value = String(summary.timeoutFactor);
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

function applyProject(opened: ProjectOpen, path: string | null): void {
  dataGen += 1;
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
  state.compareOn = Boolean(state.comparePath) && !opened.warnings.some((warning) => warning.startsWith("Compare"));
  els.compareOffset.value = String(state.compareOffsetUs / 1000);
  clearError();
  if (opened.warnings.length) setError(opened.warnings.join(" "), "project");
  void refreshCursors();
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
    math: state.math,
    triggers: state.triggers,
    notes: state.notes,
    cursorAUs: state.cursorA,
    cursorBUs: state.cursorB,
    comparePath: state.comparePath,
    compareOffsetUs: state.compareOffsetUs,
    cluster: state.cluster as Record<string, string>,
    timeoutFactor: state.summary?.timeoutFactor,
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
  const path = await pick([
    { name: "Logs", extensions: ["slog", "slbin", "csv", "txt", "log", "asc", "blf"] },
  ]);
  if (!path) return;
  try {
    adoptSummary(await withIndex(`Indexing ${basename(path)}`, () => api.beginOpen(path)), "fresh");
  } catch (err) {
    setError(errText(err), "action");
  }
}

async function openMap(): Promise<void> {
  if (!api.inTauri()) {
    els.fileMap.click();
    return;
  }
  const path = await pick([{ name: "Signal map", extensions: ["dbc", "json"] }]);
  if (!path) return;
  try {
    adoptSummary(await withIndex(`Decoding ${basename(path)}`, () => api.beginMap(path)), "keep");
    state.dirty = true;
    renderChrome();
  } catch (err) {
    setError(errText(err), "action");
  }
}

async function addMap(): Promise<void> {
  if (!api.inTauri()) {
    els.fileMap.dataset.mode = "add";
    els.fileMap.click();
    return;
  }
  const path = await pick([{ name: "Signal map", extensions: ["dbc", "json"] }]);
  if (!path) return;
  try {
    adoptSummary(
      await withIndex(`Adding ${basename(path)}`, () => api.beginAddMap(path, mapChannel())),
      "keep",
    );
    state.dirty = true;
    renderChrome();
  } catch (err) {
    setError(errText(err), "action");
  }
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
    setError("Nothing to save yet.", "action");
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

function download(name: string, text: string, type = "application/json"): void {
  const blob = new Blob([text], { type });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  // Revoking at once can cancel the download before it starts.
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

async function pick(filters: { name: string; extensions: string[] }[]): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ multiple: false, filters });
  if (typeof picked === "string") return picked;
  if (Array.isArray(picked)) return picked[0] ?? null;
  return null;
}

async function pickExport(kind: "csv" | "slog", suggested: string): Promise<string | null> {
  const { save } = await import("@tauri-apps/plugin-dialog");
  return save({
    defaultPath: suggested,
    filters: [
      kind === "csv"
        ? { name: "CSV", extensions: ["csv"] }
        : { name: "Signal Loom log", extensions: ["slog"] },
    ],
  });
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
      setNotice(direction === "next" ? "End of log." : "Start of log.");
      return;
    }
    clearError("action");
    state.playhead = frame.tUs;
    state.frame = frame;
    state.dirty = true;
    draw();
    scheduleRefresh();
  } catch (err) {
    setError(errText(err), "action");
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

function zoom(factor: number, anchor: number | null = null): void {
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
  draw();
  scheduleRefresh();
}

function exportWindow(): { t0: number; t1: number } | null {
  if (!state.summary) return null;
  if (state.cursorA != null && state.cursorB != null) {
    return { t0: Math.min(state.cursorA, state.cursorB), t1: Math.max(state.cursorA, state.cursorB) };
  }
  const view = windowFor(state.playhead, state.span, state.summary);
  return { t0: view.t0, t1: view.t1 };
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
  if (els.help.open) return;
  if (typingTarget(event.target)) return;
  if (event.key === "?" || (event.shiftKey && event.key === "/")) {
    event.preventDefault();
    els.help.showModal();
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
  if (event.key === "n" || event.key === "N") {
    event.preventDefault();
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

async function ingestFile(file: File, mode: "replace" | "add" = "replace"): Promise<void> {
  const name = file.name.toLowerCase();
  await withBusy(`Indexing ${file.name}`, async () => {
    if (name.endsWith(".loom")) {
      applyProject(await api.openProjectJson(await file.text()), null);
      return;
    }
    if (name.endsWith(".dbc") || (name.endsWith(".json") && mode === "add")) {
      const text = await file.text();
      const summary = mode === "add"
        ? await api.addMapJson(text, mapChannel())
        : await api.openMapJson(text);
      adoptSummary(summary, "keep");
      state.dirty = true;
      renderChrome();
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
  if (lower.endsWith(".loom")) {
    await withBusy(`Opening ${basename(path)}`, async () => {
      applyProject(await api.openProjectPath(path), path);
    });
    return;
  }
  try {
    if (lower.endsWith(".dbc") || lower.endsWith(".json")) {
      adoptSummary(await withIndex(`Decoding ${basename(path)}`, () => api.beginMap(path)), "keep");
      state.dirty = true;
      renderChrome();
      return;
    }
    adoptSummary(await withIndex(`Indexing ${basename(path)}`, () => api.beginOpen(path)), "fresh");
  } catch (err) {
    setError(errText(err), "action");
  }
}

function showTab(tab: RailTab): void {
  state.tab = tab;
  for (const name of ["marks", "notes", "math", "alerts", "drive"] as const) {
    const panel = document.getElementById(`panel-${name}`);
    if (panel) panel.hidden = name !== tab;
    document.querySelector(`[data-tab="${name}"]`)?.classList.toggle("is-on", name === tab);
  }
}

function renderNotes(): void {
  els.noteList.replaceChildren();
  if (state.notes.length === 0) {
    const note = document.createElement("p");
    note.className = "empty-note";
    note.textContent = "N focuses a note at the playhead";
    els.noteList.append(note);
    return;
  }
  for (const note of state.notes) {
    const row = document.createElement("div");
    row.className = "mark-row-wrap";
    const button = document.createElement("button");
    button.type = "button";
    button.className = "mark-row";
    const time = document.createElement("span");
    time.className = "mark-t";
    time.textContent = formatUs(note.tUs);
    const label = document.createElement("span");
    label.className = "mark-l";
    label.textContent = note.body;
    button.append(time, label);
    button.addEventListener("click", () => scrubTo(note.tUs));
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "mark-x";
    remove.textContent = "×";
    remove.setAttribute("aria-label", "Remove note");
    remove.addEventListener("click", () => {
      state.notes = state.notes.filter((item) => item.id !== note.id);
      state.dirty = true;
      renderNotes();
      draw();
    });
    const line = document.createElement("div");
    line.style.display = "flex";
    button.style.flex = "1";
    line.append(button, remove);
    row.append(line);
    els.noteList.append(row);
  }
}

function renderMath(): void {
  els.mathList.replaceChildren();
  if (state.math.length === 0) {
    const note = document.createElement("p");
    note.className = "empty-note";
    note.textContent = "No derived channels";
    els.mathList.append(note);
    return;
  }
  for (const channel of state.math) {
    const row = document.createElement("div");
    row.style.display = "flex";
    const button = document.createElement("button");
    button.type = "button";
    button.className = "mark-row";
    button.style.flex = "1";
    const label = document.createElement("span");
    label.className = "mark-l";
    label.textContent = channel.unit ? `${channel.name} = ${channel.expr} ${channel.unit}` : `${channel.name} = ${channel.expr}`;
    button.append(label);
    button.addEventListener("click", () => {
      if (state.plotted.includes(channel.name)) return;
      setPlotted([...state.plotted, channel.name]);
      state.dirty = true;
      renderChrome();
      draw();
      void refresh();
      void refreshOverview();
    });
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "mark-x";
    remove.textContent = "×";
    remove.addEventListener("click", () => void removeMath(channel.name));
    row.append(button, remove);
    els.mathList.append(row);
  }
}

function renderTriggers(): void {
  els.trigList.replaceChildren();
  if (state.triggers.length === 0) {
    const note = document.createElement("p");
    note.className = "empty-note";
    note.textContent = "Thresholds land on the event lane";
    els.trigList.append(note);
    return;
  }
  for (const trigger of state.triggers) {
    const row = document.createElement("div");
    row.style.display = "flex";
    const label = document.createElement("span");
    label.className = "mark-l";
    label.style.flex = "1";
    label.style.padding = "3px 4px";
    label.textContent = `${trigger.signal} ${trigger.op} ${trigger.value}`;
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "mark-x";
    remove.textContent = "×";
    remove.addEventListener("click", () => void removeTrigger(trigger.id));
    row.append(label, remove);
    els.trigList.append(row);
  }
}

function renderCompare(): void {
  if (!state.compareOn) {
    els.compareLabel.textContent = "No second drive";
    return;
  }
  const name = state.comparePath ? basename(state.comparePath) : "uploaded log";
  els.compareLabel.textContent = `${name} · offset ${state.compareOffsetUs / 1000} ms`;
}

function addNote(): void {
  if (!state.summary) return;
  const body = els.noteBody.value.trim();
  if (!body) return;
  const note: Note = {
    id: `n-${Date.now().toString(36)}`,
    tUs: Math.round(state.playhead),
    body,
  };
  state.notes = [...state.notes, note].sort((a, b) => a.tUs - b.tUs);
  state.dirty = true;
  els.noteBody.value = "";
  renderNotes();
  draw();
}

async function addMath(): Promise<void> {
  const name = (document.getElementById("math-name") as HTMLInputElement).value.trim();
  const expr = (document.getElementById("math-expr") as HTMLInputElement).value.trim();
  const unit = (document.getElementById("math-unit") as HTMLInputElement).value.trim();
  if (!name || !expr) {
    setError("A math channel needs a name and an expression.", "action");
    return;
  }
  await serialDeck(() =>
    withBusy("Compiling math", async () => {
      const next = [...state.math.filter((channel) => channel.name !== name), { name, unit, expr }];
      const summary = await api.setMath(next);
      state.math = next;
      if (!state.plotted.includes(name)) setPlotted([...state.plotted, name]);
      adoptSummary(summary, "keep");
      state.dirty = true;
      renderChrome();
      (document.getElementById("math-name") as HTMLInputElement).value = "";
      (document.getElementById("math-expr") as HTMLInputElement).value = "";
    }),
  );
}

async function removeMath(name: string): Promise<void> {
  await serialDeck(() =>
    withBusy("Compiling math", async () => {
      const next = state.math.filter((channel) => channel.name !== name);
      const summary = await api.setMath(next);
      state.math = next;
      setPlotted(state.plotted.filter((plotted) => plotted !== name));
      adoptSummary(summary, "keep");
      state.dirty = true;
      renderChrome();
    }),
  );
}

async function addTrigger(): Promise<void> {
  const signal = (document.getElementById("trig-signal") as HTMLInputElement).value.trim();
  const op = (document.getElementById("trig-op") as HTMLSelectElement).value;
  const value = Number((document.getElementById("trig-value") as HTMLInputElement).value);
  if (!signal || !Number.isFinite(value)) {
    setError("A trigger needs a signal and a finite level.", "action");
    return;
  }
  const trigger: ThresholdTrigger = { id: `t-${Date.now().toString(36)}`, signal, op, value };
  await syncTriggers((current) => [...current, trigger]);
}

async function removeTrigger(id: string): Promise<void> {
  await syncTriggers((current) => current.filter((item) => item.id !== id));
}

function timeoutInput(): HTMLInputElement {
  return document.getElementById("timeout-factor") as HTMLInputElement;
}

async function applyTimeoutFactor(): Promise<void> {
  const factor = Number(timeoutInput().value);
  const current = state.summary?.timeoutFactor;
  if (!state.summary || factor === current) return;
  if (!Number.isFinite(factor) || factor < 1 || factor > 100) {
    setError("The timeout must be between 1 and 100 cycle times.", "action");
    timeoutInput().value = String(current);
    return;
  }
  await serialDeck(async () => {
    await withBusy("Re-indexing timeouts", async () => {
      adoptSummary(await api.setTimeoutFactor(factor), "keep");
      state.dirty = true;
      renderChrome();
    });
    // A refused value leaves the field showing the factor in effect.
    timeoutInput().value = String(state.summary?.timeoutFactor ?? current);
  });
}

async function syncTriggers(change: (current: ThresholdTrigger[]) => ThresholdTrigger[]): Promise<void> {
  await serialDeck(() =>
    withBusy("Arming triggers", async () => {
      const next = change(state.triggers);
      const summary = await api.setTriggers(next);
      state.triggers = next;
      adoptSummary(summary, "keep");
      state.dirty = true;
      renderChrome();
    }),
  );
}

async function openCompare(): Promise<void> {
  if (!api.inTauri()) {
    els.fileCompare.click();
    return;
  }
  const path = await pick([
    { name: "Logs", extensions: ["slog", "slbin", "csv", "txt", "log", "asc", "blf"] },
  ]);
  if (!path) return;
  await serialDeck(() =>
    withBusy(`Comparing ${basename(path)}`, async () => {
      const summary = await api.openComparePath(path);
      state.compareOn = true;
      state.comparePath = path;
      adoptSummary(summary, "keep");
      state.dirty = true;
      renderChrome();
    }),
  );
}

async function ingestCompareFile(file: File): Promise<void> {
  await serialDeck(() =>
    withBusy(`Comparing ${file.name}`, async () => {
      const summary = await api.openCompareBytes(await file.arrayBuffer());
      state.compareOn = true;
      state.comparePath = file.name;
      adoptSummary(summary, "keep");
      state.dirty = true;
      renderChrome();
    }),
  );
}

async function clearCompareDrive(): Promise<void> {
  await serialDeck(() =>
    withBusy("Clearing compare", async () => {
      const summary = await api.clearCompare();
      state.compareOn = false;
      state.comparePath = null;
      state.compareOffsetUs = 0;
      els.compareOffset.value = "0";
      adoptSummary(summary, "keep");
      state.dirty = true;
      renderChrome();
    }),
  );
}

async function applyOffset(): Promise<void> {
  const ms = Number(els.compareOffset.value);
  if (!Number.isFinite(ms)) return;
  const offsetUs = Math.round(ms * 1000);
  if (!state.compareOn) {
    state.compareOffsetUs = offsetUs;
    state.dirty = true;
    renderCompare();
    return;
  }
  await serialDeck(async () => {
    await withBusy("Aligning drives", async () => {
      const summary = await api.setCompareOffset(offsetUs);
      state.compareOffsetUs = offsetUs;
      adoptSummary(summary, "keep");
      state.dirty = true;
      renderChrome();
    });
    // A refused offset leaves the field showing the offset in effect.
    els.compareOffset.value = String(state.compareOffsetUs / 1000);
  });
}

function dropCursor(which: "a" | "b"): void {
  if (!state.summary) return;
  const t = Math.round(state.playhead);
  if (which === "a") state.cursorA = t;
  else state.cursorB = t;
  state.dirty = true;
  void refreshCursors();
  draw();
}

async function refreshCursors(): Promise<void> {
  const seq = ++cursorSeq;
  const gen = dataGen;
  const a = state.cursorA;
  const b = state.cursorB;
  if (a == null && b == null) {
    state.cursorText = "";
    renderTransport();
    return;
  }
  if (a == null || b == null || state.plotted.length === 0) {
    const mark = a ?? b ?? 0;
    state.cursorText = `${a == null ? "B" : "A"} ${formatUs(mark)}`;
    renderTransport();
    draw();
    return;
  }
  const t0 = Math.min(a, b);
  const t1 = Math.max(a, b);
  const name = state.plotted[0];
  let text: string;
  try {
    const stats = await api.signalStats(name, t0, t1);
    text = `Δt ${formatSpan(t1 - t0)} · ${name} Δ ${formatValue(stats.last - stats.first)} · min ${formatValue(stats.min)} max ${formatValue(stats.max)} avg ${formatValue(stats.avg)}`;
  } catch (err) {
    text = errText(err);
  }
  if (seq !== cursorSeq || gen !== dataGen) return;
  state.cursorText = text;
  renderTransport();
  draw();
}

async function exportRange(kind: "csv" | "slog"): Promise<void> {
  const window = exportWindow();
  if (!window || !state.summary) {
    setError("Open a log before exporting.", "action");
    return;
  }
  const names = state.plotted.length ? state.plotted : state.summary.signals.slice(0, 1).map((signal) => signal.name);
  const suggested = `signal-loom-${Math.round(window.t0)}-${Math.round(window.t1)}.${kind}`;
  if (api.inTauri()) {
    // The desktop webview has no download UI: ask where, and let Rust write it.
    let path = await pickExport(kind, suggested);
    if (!path) return;
    if (!path.toLowerCase().endsWith(`.${kind}`)) path += `.${kind}`;
    const target = path;
    await withBusy(kind === "csv" ? "Exporting CSV" : "Trimming log", async () => {
      const bytes =
        kind === "csv"
          ? await api.saveCsv(target, names, window.t0, window.t1)
          : await api.saveSlog(target, window.t0, window.t1);
      setNotice(`Saved ${basename(target)} (${formatBytes(bytes)})`);
    });
    return;
  }
  await withBusy(kind === "csv" ? "Exporting CSV" : "Trimming log", async () => {
    if (kind === "csv") {
      download(suggested, await api.exportCsv(names, window.t0, window.t1), "text/csv");
    } else {
      download(suggested, await api.exportSlog(window.t0, window.t1), "text/plain");
    }
  });
}

async function captureBus(): Promise<void> {
  if (!els.captureArm.checked) {
    setError("Tick Listen only before a SocketCAN capture. The socket is read-only.", "action");
    return;
  }
  const iface = els.captureIface.value.trim();
  const durationMs = Math.round(Number(els.captureMs.value));
  await withBusy(`Listening on ${iface}`, async () => {
    adoptSummary(await api.captureCan(iface, durationMs), "fresh");
  });
}

/** The slot under a mouse event on the cluster canvas. */
function slotFromEvent(event: MouseEvent): SlotId | null {
  const rect = els.gauges.getBoundingClientRect();
  return slotAt(event.clientX - rect.left, event.clientY - rect.top, rect.width, rect.height);
}

function closeSlotChooser(): void {
  (document.getElementById("slot-chooser") as HTMLElement).hidden = true;
}

/** Let the user pick the signal for one gauge or lamp: any signal with data, or the default. */
function openSlotChooser(slot: SlotId, clientX: number, clientY: number): void {
  if (!state.summary) return;
  const chooser = document.getElementById("slot-chooser") as HTMLElement;
  const select = document.getElementById("slot-signal") as HTMLSelectElement;
  (document.getElementById("slot-title") as HTMLElement).textContent = slotSpec(slot).title;
  const fallback = document.createElement("option");
  fallback.value = "";
  fallback.textContent = `Default (${slotSpec(slot).defaults[0]})`;
  const options = [fallback];
  const signals = state.summary.signals
    .filter(hasData)
    .sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
  for (const signal of signals) {
    const option = document.createElement("option");
    option.value = signal.name;
    option.textContent = signal.messageName ? `${signal.name} · ${signal.messageName}` : signal.name;
    options.push(option);
  }
  select.replaceChildren(...options);
  select.value = state.cluster[slot] ?? "";
  select.onchange = () => {
    if (select.value) state.cluster = { ...state.cluster, [slot]: select.value };
    else {
      const next = { ...state.cluster };
      delete next[slot];
      state.cluster = next;
    }
    state.dirty = true;
    closeSlotChooser();
    renderChrome();
    draw();
  };
  chooser.style.left = `${Math.min(clientX, window.innerWidth - 240)}px`;
  chooser.style.top = `${clientY + 12}px`;
  chooser.hidden = false;
  select.focus();
}

function bind(): void {
  els.gauges.addEventListener("click", (event) => {
    const slot = slotFromEvent(event);
    if (slot) openSlotChooser(slot, event.clientX, event.clientY);
  });
  els.gauges.addEventListener("mousemove", (event) => {
    els.gauges.classList.toggle("on-slot", state.summary != null && slotFromEvent(event) != null);
  });
  document.addEventListener("mousedown", (event) => {
    const chooser = document.getElementById("slot-chooser") as HTMLElement;
    if (!chooser.hidden && !chooser.contains(event.target as Node) && event.target !== els.gauges) closeSlotChooser();
  });
  document.getElementById("slot-signal")?.addEventListener("keydown", (event) => {
    if (event.key === "Escape") closeSlotChooser();
  });
  els.runtime.textContent = api.inTauri() ? "Desktop" : "Browser preview";
  document.getElementById("btn-sample")?.addEventListener("click", () => void loadSample());
  document.getElementById("btn-open")?.addEventListener("click", () => void openLog());
  document.getElementById("btn-map")?.addEventListener("click", () => void openMap());
  document.getElementById("btn-add-map")?.addEventListener("click", () => void addMap());
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
    const mode = els.fileMap.dataset.mode === "add" ? "add" : "replace";
    els.fileMap.dataset.mode = "replace";
    els.fileMap.value = "";
    if (file) void ingestFile(file, mode);
  });
  els.fileProject.addEventListener("change", () => {
    const file = els.fileProject.files?.[0];
    els.fileProject.value = "";
    if (file) void ingestFile(file);
  });
  els.fileCompare.addEventListener("change", () => {
    const file = els.fileCompare.files?.[0];
    els.fileCompare.value = "";
    if (file) void ingestCompareFile(file);
  });
  document.querySelectorAll<HTMLButtonElement>("[data-tab]").forEach((button) => {
    button.addEventListener("click", () => showTab(button.dataset.tab as RailTab));
  });
  document.getElementById("note-form")?.addEventListener("submit", (event) => {
    event.preventDefault();
    addNote();
  });
  document.getElementById("math-form")?.addEventListener("submit", (event) => {
    event.preventDefault();
    void addMath();
  });
  document.getElementById("trig-form")?.addEventListener("submit", (event) => {
    event.preventDefault();
    void addTrigger();
  });
  timeoutInput().addEventListener("change", () => void applyTimeoutFactor());
  document.getElementById("btn-compare")?.addEventListener("click", () => void openCompare());
  document.getElementById("btn-compare-clear")?.addEventListener("click", () => void clearCompareDrive());
  els.compareOffset.addEventListener("change", () => void applyOffset());
  document.getElementById("cursor-a")?.addEventListener("click", () => dropCursor("a"));
  document.getElementById("cursor-b")?.addEventListener("click", () => dropCursor("b"));
  document.getElementById("cursor-clear")?.addEventListener("click", () => {
    state.cursorA = null;
    state.cursorB = null;
    state.cursorText = "";
    cursorSeq += 1;
    state.dirty = true;
    draw();
  });
  document.getElementById("export-csv")?.addEventListener("click", () => void exportRange("csv"));
  document.getElementById("export-slog")?.addEventListener("click", () => void exportRange("slog"));
  els.captureArm.addEventListener("change", () => {
    els.captureBtn.disabled = !els.captureArm.checked;
  });
  els.captureBtn.addEventListener("click", () => void captureBus());

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
    draw();
  });
  els.plot.addEventListener("pointerleave", () => {
    state.hoverT = null;
    draw();
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
