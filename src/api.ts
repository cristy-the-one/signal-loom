import type { FrameHit, ProjectFile, ProjectOpen, Query, Series, Summary } from "./types";

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

export function openProjectJson(json: string): Promise<ProjectOpen> {
  return http<ProjectOpen>("/api/open-project", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ json }),
  });
}

export function writeProject(path: string, project: ProjectFile): Promise<void> {
  if (inTauri()) return invoke<void>("write_project", { path, project });
  return http<void>("/api/write-project", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path, json: JSON.stringify(project) }),
  });
}
