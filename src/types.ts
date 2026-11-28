export interface Summary {
  logLabel: string;
  logPath: string | null;
  mapLabel: string | null;
  mapPath: string | null;
  format: string;
  frameCount: number;
  eventCount: number;
  eventsTruncated: boolean;
  checkpointCount: number;
  tStartUs: number;
  tEndUs: number;
  bytes: number;
  signals: SignalInfo[];
  events: LogEvent[];
  skippedRecords: number;
  warnings: string[];
  /** Cycle times without a frame before a message is marked late. */
  timeoutFactor: number;
  /** How many of the map's messages appear in this log. */
  mapMatch: { matched: number; total: number } | null;
}

/** What went wrong, as the engine reports it on both transports. */
export type ErrorKind = "not_found" | "invalid" | "parse" | "binary" | "cancelled" | "io" | "internal";

export interface IndexStatus {
  running: boolean;
  done: boolean;
  idle: boolean;
  bytesDone: number;
  bytesTotal: number;
  frames: number;
  skipped: number;
  summary?: Summary;
  /** Set when the finished job opened a project: the project as loaded and what could not be opened. */
  project?: { project: ProjectFile; warnings: string[]; compareOpened: boolean };
  error?: string;
  /** Set with `error`: `cancelled` when the user stopped the job. */
  errorKind?: ErrorKind;
}

export interface SignalInfo {
  name: string;
  definedName?: string;
  unit: string;
  messageName: string;
  messageId: number | null;
  channel?: number;
  min: number | null;
  max: number | null;
  step: number | null;
  fromMap: boolean;
  labels?: { value: number; label: string }[];
}

export interface LogEvent {
  tUs: number;
  label: string;
}

export interface Series {
  name: string;
  unit: string;
  points: Point[];
}

export interface Point {
  t: number;
  v: number;
}

export interface ValueRead {
  name: string;
  unit: string;
  value: number;
  label?: string;
}

export interface FrameHit {
  tUs: number;
  ordinal: number;
  messageId: number | null;
  messageName: string;
  extended: boolean;
  dlc: number;
  dataHex: string;
  values: ValueRead[];
}

export interface Bookmark {
  id: string;
  tUs: number;
  label: string;
}

export interface MathChannel {
  name: string;
  unit: string;
  expr: string;
}

export interface ThresholdTrigger {
  id: string;
  signal: string;
  op: string;
  value: number;
}

export interface Note {
  id: string;
  tUs: number;
  body: string;
}

export interface BusLoad {
  frames: number;
  rate: number;
  load: number;
}

export interface WindowStats {
  count: number;
  min: number;
  max: number;
  avg: number;
  first: number;
  last: number;
}

export type WidgetKind = "arc" | "bar" | "digit" | "value" | "lamp";
export type LampOp = ">" | ">=" | "<" | "<=" | "==" | "!=";
export type TraceStyle = "auto" | "step" | "line" | "area" | "state" | "flag" | "heat";
export type ConcreteTraceStyle = Exclude<TraceStyle, "auto">;

export interface ClusterWidget {
  id: string;
  kind: WidgetKind;
  signal?: string | null;
  label?: string;
  color?: string;
  min?: number;
  max?: number;
  decimals?: number;
  warnFrom?: number;
  lampOp?: LampOp;
  lampThreshold?: number;
}

export interface ProjectFile {
  format: "signal-loom";
  version: 1;
  logPath: string;
  signalMapPath: string | null;
  bookmarks: Bookmark[];
  view: {
    playheadUs: number;
    spanUs: number;
    plotted: string[];
  };
  math?: MathChannel[];
  triggers?: ThresholdTrigger[];
  notes?: Note[];
  cursorAUs?: number | null;
  cursorBUs?: number | null;
  comparePath?: string | null;
  compareOffsetUs?: number;
  timeoutFactor?: number;
  /** Legacy cluster slot to the signal shown in it. */
  cluster?: Record<string, string>;
  /** Custom ordered cluster widgets. */
  clusterWidgets?: ClusterWidget[];
  /** True when the cluster widgets list was explicitly customized (including an empty strip). */
  clusterCustomized?: boolean;
  /** Per-signal scope trace visualization style overrides. */
  signalStyles?: Record<string, TraceStyle>;
  /** Per-signal custom trace colors (`#rrggbb`). */
  signalColors?: Record<string, string>;
  /** Scope track layout mode (`overlay`, `split`, `grouped`). */
  laneMode?: "overlay" | "split" | "grouped";
  /** Manual signal -> track group overrides used in `grouped` mode. */
  signalLanes?: Record<string, number>;
  /** User-arranged vertical order of lane ids. */
  laneOrder?: string[];
  /** Whether the left sidebar was collapsed. */
  railCollapsed?: boolean;
  /** Whether the cluster strip was hidden. */
  clusterHidden?: boolean;
  /** Whether checksum/counter signals were hidden in the Signals list. */
  hideHousekeeping?: boolean;
}

/**
 * What the UI owns of a saved project. The engine adds the deck it holds: log, map and compare paths,
 * compare offset, math channels, triggers and the timeout.
 */
export type ProjectView = Pick<
  ProjectFile,
  | "view"
  | "bookmarks"
  | "notes"
  | "cursorAUs"
  | "cursorBUs"
  | "cluster"
  | "clusterWidgets"
  | "clusterCustomized"
  | "signalStyles"
  | "signalColors"
  | "laneMode"
  | "signalLanes"
  | "laneOrder"
  | "railCollapsed"
  | "clusterHidden"
  | "hideHousekeeping"
>;

export interface ProjectOpen {
  project: ProjectFile;
  summary: Summary;
  warnings: string[];
  /** Whether the project's compare log is open; the warnings say why not. */
  compareOpened: boolean;
}

export interface Query {
  t0Us: number;
  t1Us: number;
  signals: string[];
  maxPoints: number;
  includeCompare?: boolean;
}
