import type {
  BusLoad,
  FrameHit,
  MathChannel,
  ProjectView,
  Query,
  Series,
  ErrorKind,
  IndexStatus,
  Summary,
  ThresholdTrigger,
  ValueRead,
  WindowStats,
} from "./types";

export function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** A failure from the engine on either transport: what went wrong, and the text to show. */
export class ApiError extends Error {
  readonly kind: ErrorKind;

  constructor(kind: ErrorKind, message: string) {
    super(message);
    this.name = "ApiError";
    this.kind = kind;
  }
}

const ERROR_KINDS: readonly string[] = ["not_found", "invalid", "parse", "binary", "cancelled", "io", "internal"];

function isErrorBody(value: unknown): value is { kind: ErrorKind; message: string } {
  return (
    typeof value === "object" &&
    value !== null &&
    "kind" in value &&
    typeof value.kind === "string" &&
    ERROR_KINDS.includes(value.kind) &&
    "message" in value &&
    typeof value.message === "string"
  );
}

/**
 * The one place a transport failure becomes an `ApiError`. The engine sends `{kind, message}`
 * (the Tauri rejection value, or the body of an HTTP error); anything else is Tauri's own text.
 */
function toApiError(raw: unknown): ApiError {
  if (raw instanceof ApiError) return raw;
  if (isErrorBody(raw)) return new ApiError(raw.kind, raw.message);
  if (typeof raw === "string") return new ApiError("internal", raw);
  if (raw instanceof Error) return new ApiError("internal", raw.message);
  return new ApiError("internal", "Something went wrong");
}

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke: call } = await import("@tauri-apps/api/core");
  try {
    return await call<T>(command, args);
  } catch (err) {
    throw toApiError(err);
  }
}

async function http<T>(path: string, init?: RequestInit): Promise<T> {
  let res: Response;
  let text: string;
  try {
    res = await fetch(path, init);
    text = await res.text();
  } catch (err) {
    throw new ApiError("io", err instanceof Error ? err.message : "Could not reach the engine");
  }
  let data: unknown = null;
  if (text) {
    try {
      data = JSON.parse(text) as unknown;
    } catch {
      data = text;
    }
  }
  if (!res.ok) throw toApiError(isErrorBody(data) ? data : text || res.statusText);
  return data as T;
}

export async function health(): Promise<boolean> {
  try {
    const res = await fetch("/api/health");
    return res.ok;
  } catch {
    return false;
  }
}

export function openSample(): Promise<Summary> {
  if (inTauri()) return invoke<Summary>("open_sample");
  return http<Summary>("/api/open-sample", { method: "POST" });
}

export function beginOpen(path: string): Promise<void> {
  if (inTauri()) return invoke<void>("begin_open_log", { path });
  return http<void>("/api/begin-open", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path }),
  });
}

export function beginMap(path: string): Promise<void> {
  if (inTauri()) return invoke<void>("begin_open_map", { path });
  return http<void>("/api/begin-map", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path }),
  });
}

export function beginAddMap(path: string, channel: number): Promise<void> {
  if (inTauri()) return invoke<void>("begin_add_map", { path, channel });
  return http<void>("/api/begin-add-map", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path, channel }),
  });
}

export function beginProject(path: string): Promise<void> {
  if (inTauri()) return invoke<void>("begin_open_project", { path });
  return http<void>("/api/begin-project", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path }),
  });
}

/**
 * Browser preview: the upload has no folder, so the engine reports a relative log,
 * map or compare path as unresolved instead of looking in its own directory.
 */
export function beginProjectJson(json: string): Promise<void> {
  return http<void>("/api/begin-project-json", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ json }),
  });
}

export function beginCompare(path: string): Promise<void> {
  if (inTauri()) return invoke<void>("begin_open_compare", { path });
  return http<void>("/api/begin-compare", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path }),
  });
}

/** Listens on a SocketCAN interface as a background job; cancel ends the capture early. */
export function beginCapture(iface: string, durationMs: number): Promise<void> {
  const body = { iface, durationMs };
  if (inTauri()) return invoke<void>("begin_capture_can", body);
  return http<void>("/api/begin-capture", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

export function indexProgress(): Promise<IndexStatus> {
  if (inTauri()) return invoke<IndexStatus>("index_progress");
  return http<IndexStatus>("/api/progress");
}

export function cancelIndex(): Promise<IndexStatus> {
  if (inTauri()) return invoke<IndexStatus>("cancel_index");
  return http<IndexStatus>("/api/cancel", { method: "POST" });
}

export function openBytes(name: string, bytes: ArrayBuffer): Promise<Summary> {
  return http<Summary>("/api/open-bytes", {
    method: "POST",
    headers: { "x-filename": name },
    body: bytes,
  });
}

export function addMapJson(json: string, channel: number): Promise<Summary> {
  return http<Summary>("/api/add-map", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ json, channel }),
  });
}

export function openMapJson(json: string): Promise<Summary> {
  return http<Summary>("/api/open-map", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ json }),
  });
}

function us(n: number): number {
  if (!Number.isFinite(n) || n <= 0) return 0;
  return Math.round(n);
}

export function query(q: Query): Promise<Series[]> {
  const query = {
    ...q,
    t0Us: us(q.t0Us),
    t1Us: us(q.t1Us),
    maxPoints: Math.max(1, Math.round(q.maxPoints)),
  };
  if (inTauri()) return invoke<Series[]>("query_series", { query });
  return http<Series[]>("/api/query", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(query),
  });
}

export function frameAt(tUs: number): Promise<FrameHit | null> {
  const t = us(tUs);
  if (inTauri()) return invoke<FrameHit | null>("frame_at", { tUs: t });
  return http<FrameHit | null>("/api/frame", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ tUs: t }),
  });
}

export function step(tUs: number, direction: "next" | "prev"): Promise<FrameHit | null> {
  const t = us(tUs);
  if (inTauri()) return invoke<FrameHit | null>("step_frame", { tUs: t, direction });
  return http<FrameHit | null>("/api/step", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ tUs: t, direction }),
  });
}

export function valuesAt(tUs: number): Promise<ValueRead[]> {
  const t = us(tUs);
  if (inTauri()) return invoke<ValueRead[]>("values_at", { tUs: t });
  return http<ValueRead[]>("/api/values", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ tUs: t }),
  });
}

export function busLoad(t0Us: number, t1Us: number): Promise<BusLoad> {
  const body = { t0Us: us(t0Us), t1Us: us(t1Us) };
  if (inTauri()) return invoke<BusLoad>("bus_load", body);
  return http<BusLoad>("/api/bus", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

export function signalStats(name: string, t0Us: number, t1Us: number): Promise<WindowStats> {
  const body = { name, t0Us: us(t0Us), t1Us: us(t1Us) };
  if (inTauri()) return invoke<WindowStats>("signal_stats", body);
  return http<WindowStats>("/api/stats", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

/** A text export. `truncated` means the row cap cut it short after `rows` data rows. */
export interface ExportResult {
  text: string;
  rows: number;
  truncated: boolean;
}

/** What a desktop save wrote: its size, and the same row facts as an export. */
export interface SaveResult {
  bytes: number;
  rows: number;
  truncated: boolean;
}

export function exportCsv(names: string[], t0Us: number, t1Us: number): Promise<ExportResult> {
  return exportCall("/api/export-csv", "export_csv", { names, t0Us: us(t0Us), t1Us: us(t1Us) });
}

export function exportSlog(t0Us: number, t1Us: number): Promise<ExportResult> {
  return exportCall("/api/export-slog", "export_slog", { t0Us: us(t0Us), t1Us: us(t1Us) });
}

export function setMath(channels: MathChannel[]): Promise<Summary> {
  if (inTauri()) return invoke<Summary>("set_math", { channels });
  return http<Summary>("/api/math", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ channels }),
  });
}

export function setTimeoutFactor(factor: number): Promise<Summary> {
  if (inTauri()) return invoke<Summary>("set_timeout_factor", { factor });
  return http<Summary>("/api/timeout-factor", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ factor }),
  });
}

export function setTriggers(triggers: ThresholdTrigger[]): Promise<Summary> {
  if (inTauri()) return invoke<Summary>("set_triggers", { triggers });
  return http<Summary>("/api/triggers", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ triggers }),
  });
}

export function openCompareBytes(bytes: ArrayBuffer): Promise<Summary> {
  return http<Summary>("/api/compare-bytes", {
    method: "POST",
    body: bytes,
  });
}

export function setCompareOffset(offsetUs: number): Promise<Summary> {
  const body = { offsetUs: Math.round(offsetUs) };
  if (inTauri()) return invoke<Summary>("set_compare_offset", body);
  return http<Summary>("/api/compare-offset", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

export function clearCompare(): Promise<Summary> {
  if (inTauri()) return invoke<Summary>("clear_compare");
  return http<Summary>("/api/compare-clear", { method: "POST" });
}

async function exportCall(path: string, command: string, args: Record<string, unknown>): Promise<ExportResult> {
  if (inTauri()) return invoke<ExportResult>(command, args);
  return http<ExportResult>(path, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(args),
  });
}

/** Desktop only: write a CSV export to `path` (.csv). Resolves to the bytes written and the row facts. */
export function saveCsv(path: string, names: string[], t0Us: number, t1Us: number): Promise<SaveResult> {
  return invoke<SaveResult>("save_csv", { path, names, t0Us: us(t0Us), t1Us: us(t1Us) });
}

/** Desktop only: write the trimmed log to `path` (.slog). Resolves to the bytes written and the row facts. */
export function saveSlog(path: string, t0Us: number, t1Us: number): Promise<SaveResult> {
  return invoke<SaveResult>("save_slog", { path, t0Us: us(t0Us), t1Us: us(t1Us) });
}

/** Desktop: the engine writes the project to `path`, composing the deck from its own state and `view`. */
export function writeProject(path: string, view: ProjectView): Promise<void> {
  if (!inTauri()) return Promise.reject(new ApiError("invalid", "Saving to a path needs the desktop app."));
  return invoke<void>("write_project", { path, view });
}

/** Browser preview: the project as the engine would save it, as JSON text to download. */
export async function projectJson(view: ProjectView): Promise<string> {
  const reply = await http<{ json: string }>("/api/project-json", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(view),
  });
  return reply.json;
}
