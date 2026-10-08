/** Every element the app touches, resolved once at startup; a missing or mistyped id stops the app with the full list. */

type Ctor<T extends HTMLElement> = { new (): T; prototype: T };

const problems: string[] = [];

function must<T extends HTMLElement>(type: Ctor<T>, id: string): T {
  const node = document.getElementById(id);
  if (node instanceof type) return node;
  problems.push(node ? `#${id} is not a ${type.name}` : `#${id} is missing`);
  return null as never;
}

function mustQuery<T extends HTMLElement>(type: Ctor<T>, selector: string): T {
  const node = document.querySelector(selector);
  if (node instanceof type) return node;
  problems.push(node ? `${selector} is not a ${type.name}` : `${selector} is missing`);
  return null as never;
}

const any = (id: string) => must(HTMLElement, id);
const button = (id: string) => must(HTMLButtonElement, id);
const input = (id: string) => must(HTMLInputElement, id);
const canvas = (id: string) => must(HTMLCanvasElement, id);
const form = (id: string) => must(HTMLFormElement, id);
const select = (id: string) => must(HTMLSelectElement, id);

export const RAIL_TABS = ["marks", "notes", "math", "alerts", "drive"] as const;
export type RailTab = (typeof RAIL_TABS)[number];

export const els = {
  projectName: any("project-name"),
  logName: any("log-name"),
  logMeta: any("log-meta"),
  runtime: any("runtime"),
  error: any("error"),
  warn: any("warn"),
  notice: any("notice"),
  btnSample: button("btn-sample"),
  btnOpen: button("btn-open"),
  btnMap: button("btn-map"),
  btnAddMap: button("btn-add-map"),
  btnProject: button("btn-project"),
  btnHelp: button("btn-help"),
  save: button("btn-save"),
  dbcChannel: input("dbc-channel"),
  sigCount: any("sig-count"),
  sigFilter: input("sig-filter"),
  sigList: any("sig-list"),
  markForm: form("mark-form"),
  markLabel: input("mark-label"),
  markList: any("mark-list"),
  eventList: any("event-list"),
  noteForm: form("note-form"),
  noteBody: input("note-body"),
  noteList: any("note-list"),
  mathForm: form("math-form"),
  mathName: input("math-name"),
  mathExpr: input("math-expr"),
  mathUnit: input("math-unit"),
  mathList: any("math-list"),
  trigForm: form("trig-form"),
  trigSignal: input("trig-signal"),
  trigOp: select("trig-op"),
  trigValue: input("trig-value"),
  trigList: any("trig-list"),
  timeoutFactor: input("timeout-factor"),
  btnCompare: button("btn-compare"),
  btnCompareClear: button("btn-compare-clear"),
  compareLabel: any("compare-label"),
  compareOffset: input("compare-offset"),
  cursorA: button("cursor-a"),
  cursorB: button("cursor-b"),
  cursorClear: button("cursor-clear"),
  exportCsv: button("export-csv"),
  exportSlog: button("export-slog"),
  captureArm: input("capture-arm"),
  captureIface: input("capture-iface"),
  captureMs: input("capture-ms"),
  captureBtn: button("btn-capture"),
  viewport: any("viewport"),
  gauges: canvas("gauges"),
  bus: canvas("bus"),
  plot: canvas("plot"),
  timeline: canvas("timeline"),
  crosshair: any("crosshair"),
  crossTime: any("cross-time"),
  crossVals: any("cross-vals"),
  legend: any("legend"),
  scaleNote: any("scale-note"),
  stageMsg: any("stage-msg"),
  prevEvent: button("prev-event"),
  prevFrame: button("prev-frame"),
  play: button("play"),
  nextFrame: button("next-frame"),
  nextEvent: button("next-event"),
  timeReadout: any("time-readout"),
  frameReadout: any("frame-readout"),
  spanReadout: any("span-readout"),
  cursorRead: any("cursor-read"),
  zoomOut: button("zoom-out"),
  zoomIn: button("zoom-in"),
  zoomAll: button("zoom-all"),
  rate: button("rate"),
  veil: any("veil"),
  veilLabel: any("veil-label"),
  veilDetail: any("veil-detail"),
  veilFill: any("veil-fill"),
  veilCancel: button("veil-cancel"),
  drop: any("drop"),
  help: must(HTMLDialogElement, "help"),
  slotChooser: any("slot-chooser"),
  slotTitle: any("slot-title"),
  slotSignal: select("slot-signal"),
  fileLog: input("file-log"),
  fileMap: input("file-map"),
  fileProject: input("file-project"),
  fileCompare: input("file-compare"),
};

export const tabs = Object.fromEntries(
  RAIL_TABS.map((name) => [
    name,
    { button: mustQuery(HTMLButtonElement, `[data-tab="${name}"]`), panel: any(`panel-${name}`) },
  ]),
) as Record<RailTab, { button: HTMLButtonElement; panel: HTMLElement }>;

if (problems.length > 0) throw new Error(`index.html does not match the app: ${problems.join(", ")}`);
