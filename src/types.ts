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

export interface IndexStatus {
  running: boolean;
  done: boolean;
  idle: boolean;
  bytesDone: number;
  bytesTotal: number;
  frames: number;
  skipped: number;
  summary?: Summary;
  error?: string;
}

export interface SignalInfo {
  name: string;
  unit: string;
  messageName: string;
  messageId: number | null;
  min: number | null;
  max: number | null;
  step: number | null;
  fromMap: boolean;
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
  /** Cluster slot to the signal shown in it. */
  cluster?: Record<string, string>;
}

export interface ProjectOpen {
  project: ProjectFile;
  summary: Summary;
  warnings: string[];
}

export interface Query {
  t0Us: number;
  t1Us: number;
  signals: string[];
  maxPoints: number;
  includeCompare?: boolean;
}
