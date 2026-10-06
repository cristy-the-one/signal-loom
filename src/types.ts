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
}

export interface SignalInfo {
  name: string;
  unit: string;
  messageName: string;
  messageId: number | null;
  min: number | null;
  max: number | null;
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
}

export interface FrameHit {
  tUs: number;
  ordinal: number;
  messageId: number | null;
  messageName: string;
  dlc: number;
  dataHex: string;
  values: ValueRead[];
}

export interface Bookmark {
  id: string;
  tUs: number;
  label: string;
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
}
