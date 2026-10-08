import type {
  BusLoad,
  FrameHit,
  MathChannel,
  ProjectFile,
  ProjectOpen,
  Query,
  Series,
  IndexStatus,
  Summary,
  ThresholdTrigger,
  ValueRead,
  WindowStats,
} from "./types";

export function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke: call } = await import("@tauri-apps/api/core");
  return call<T>(command, args);
}

async function http<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, init);
  const text = await res.text();
  let data: unknown = null;
  if (text) {
    try {
      data = JSON.parse(text) as unknown;
    } catch {
      data = { error: text };
    }
  }
  if (!res.ok) {
    const message =
      data && typeof data === "object" && "error" in data && typeof data.error === "string"
        ? data.error
        : res.statusText;
    throw new Error(message);
  }
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

export function openPath(path: string): Promise<Summary> {
  if (inTauri()) return invoke<Summary>("open_log", { path });
  return http<Summary>("/api/open-path", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path }),
  });
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

export function openMapPath(path: string): Promise<Summary> {
  if (inTauri()) return invoke<Summary>("open_signal_map", { path });
  return http<Summary>("/api/open-map-path", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path }),
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

export function openProjectPath(path: string): Promise<ProjectOpen> {
  if (inTauri()) return invoke<ProjectOpen>("open_project", { path });
  return http<ProjectOpen>("/api/open-project-path", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path }),
  });
}

/**
 * Browser preview: the upload has no folder, so the engine reports a relative log,
 * map or compare path as unresolved instead of looking in its own directory.
 */
export function openProjectJson(json: string): Promise<ProjectOpen> {
  return http<ProjectOpen>("/api/open-project", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ json }),
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

export function exportCsv(names: string[], t0Us: number, t1Us: number): Promise<string> {
  return textCall("/api/export-csv", "export_csv", { names, t0Us: us(t0Us), t1Us: us(t1Us) });
}

export function exportSlog(t0Us: number, t1Us: number): Promise<string> {
  return textCall("/api/export-slog", "export_slog", { t0Us: us(t0Us), t1Us: us(t1Us) });
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

export function openComparePath(path: string): Promise<Summary> {
  if (inTauri()) return invoke<Summary>("open_compare", { path });
  return http<Summary>("/api/compare-path", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path }),
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

export function captureCan(iface: string, durationMs: number): Promise<Summary> {
  const body = { iface, durationMs };
  if (inTauri()) return invoke<Summary>("capture_can", body);
  return http<Summary>("/api/capture", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

async function textCall(path: string, command: string, args: Record<string, unknown>): Promise<string> {
  if (inTauri()) return invoke<string>(command, args);
  const data = await http<{ text: string }>(path, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(args),
  });
  return data.text;
}

/** Desktop only: write a CSV export to `path` (.csv). Resolves to the bytes written. */
export function saveCsv(path: string, names: string[], t0Us: number, t1Us: number): Promise<number> {
  return invoke<number>("save_csv", { path, names, t0Us: us(t0Us), t1Us: us(t1Us) });
}

/** Desktop only: write the trimmed log to `path` (.slog). Resolves to the bytes written. */
export function saveSlog(path: string, t0Us: number, t1Us: number): Promise<number> {
  return invoke<number>("save_slog", { path, t0Us: us(t0Us), t1Us: us(t1Us) });
}

export function writeProject(path: string, project: ProjectFile): Promise<void> {
  if (!inTauri()) return Promise.reject(new Error("Saving to a path needs the desktop app."));
  return invoke<void>("write_project", { path, project });
}
