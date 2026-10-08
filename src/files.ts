import * as api from "./api";
import { els } from "./dom";
import { basename, errText } from "./format";
import { adoptEdited, adoptSummary, applyProject, currentProject } from "./project";
import { renderChrome, state } from "./state";
import { setError, withBusy, withIndex } from "./status";

export interface FileFilter {
  name: string;
  extensions: string[];
}

export const LOG_FILTERS: FileFilter[] = [
  { name: "Logs", extensions: ["slog", "slbin", "csv", "txt", "log", "asc", "blf"] },
];
const MAP_FILTERS: FileFilter[] = [{ name: "Signal map", extensions: ["dbc", "json"] }];
const PROJECT_FILTERS: FileFilter[] = [{ name: "Signal Loom project", extensions: ["loom"] }];

/**
 * Asks for a file: the native picker in the desktop shell. In the browser it opens the hidden file input
 * instead and returns null; the input's change event carries on from there.
 */
export async function pickPath(
  input: HTMLInputElement,
  filters: FileFilter[],
  mode?: "add" | "open",
): Promise<string | null> {
  if (!api.inTauri()) {
    input.dataset.mode = mode ?? "open";
    input.click();
    return null;
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ multiple: false, filters });
  if (typeof picked === "string") return picked;
  if (Array.isArray(picked)) return picked[0] ?? null;
  return null;
}

/** Asks where to write a file, in the desktop shell. */
export async function pickSavePath(defaultPath: string, filter: FileFilter): Promise<string | null> {
  const { save } = await import("@tauri-apps/plugin-dialog");
  return save({ defaultPath, filters: [filter] });
}

/** The path with the extension a save dialog may have left off. */
export function withExtension(path: string, extension: string): string {
  return path.toLowerCase().endsWith(extension) ? path : path + extension;
}

/** Hands text to the browser as a download. */
export function download(name: string, text: string, type = "application/json"): void {
  const blob = new Blob([text], { type });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  // Revoking at once can cancel the download before it starts.
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

/** Runs the file input's handler on its chosen file, then empties the input so the same file can be chosen again. */
export function onFile(input: HTMLInputElement, handle: (file: File, mode: "replace" | "add") => void): void {
  input.addEventListener("change", () => {
    const file = input.files?.[0];
    const mode = input.dataset.mode === "add" ? "add" : "replace";
    delete input.dataset.mode;
    input.value = "";
    if (file) handle(file, mode);
  });
}

export function mapChannel(): number {
  const value = Number(els.dbcChannel.value);
  if (!Number.isFinite(value) || value < 0) return 0;
  return Math.min(255, Math.round(value));
}

/** Indexes in the background, then adopts the result as a new log or as an edit to the current one. */
async function indexInto(label: string, start: () => Promise<void>, as: "fresh" | "edit"): Promise<void> {
  try {
    const summary = await withIndex(label, start);
    if (as === "fresh") adoptSummary(summary, "fresh");
    else adoptEdited(summary);
  } catch (err) {
    setError(errText(err), "action");
  }
}

export async function loadSample(): Promise<void> {
  await withBusy("Indexing cluster sample", async () => {
    adoptSummary(await api.openSample(), "fresh");
  });
}

export async function openLog(): Promise<void> {
  const path = await pickPath(els.fileLog, LOG_FILTERS);
  if (!path) return;
  await indexInto(`Indexing ${basename(path)}`, () => api.beginOpen(path), "fresh");
}

async function openMap(): Promise<void> {
  const path = await pickPath(els.fileMap, MAP_FILTERS);
  if (!path) return;
  await indexInto(`Decoding ${basename(path)}`, () => api.beginMap(path), "edit");
}

async function addMap(): Promise<void> {
  const path = await pickPath(els.fileMap, MAP_FILTERS, "add");
  if (!path) return;
  await indexInto(`Adding ${basename(path)}`, () => api.beginAddMap(path, mapChannel()), "edit");
}

export async function openProject(): Promise<void> {
  const path = await pickPath(els.fileProject, PROJECT_FILTERS);
  if (!path) return;
  await openProjectPath(path);
}

async function openProjectPath(path: string): Promise<void> {
  await withBusy(`Opening ${basename(path)}`, async () => {
    applyProject(await api.openProjectPath(path), path);
  });
}

export async function saveProject(asNew: boolean): Promise<void> {
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
  if (!path) path = await pickSavePath(state.projectPath ?? "session.loom", PROJECT_FILTERS[0]);
  if (!path) return;
  const target = withExtension(path, ".loom");
  await withBusy("Saving project", async () => {
    await api.writeProject(target, project);
    state.projectPath = target;
    state.dirty = false;
    renderChrome();
  });
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
      adoptEdited(summary);
      return;
    }
    if (name.endsWith(".json")) {
      const text = await file.text();
      if (text.includes('"format"') && text.includes("signal-loom")) {
        applyProject(await api.openProjectJson(text), null);
      } else {
        adoptEdited(await api.openMapJson(text));
      }
      return;
    }
    adoptSummary(await api.openBytes(file.name, await file.arrayBuffer()), "fresh");
  });
}

async function ingestPath(path: string): Promise<void> {
  const lower = path.toLowerCase();
  if (lower.endsWith(".loom")) {
    await openProjectPath(path);
    return;
  }
  if (lower.endsWith(".dbc") || lower.endsWith(".json")) {
    await indexInto(`Decoding ${basename(path)}`, () => api.beginMap(path), "edit");
    return;
  }
  await indexInto(`Indexing ${basename(path)}`, () => api.beginOpen(path), "fresh");
}

export function bindFiles(): void {
  els.btnSample.addEventListener("click", () => void loadSample());
  els.btnOpen.addEventListener("click", () => void openLog());
  els.btnMap.addEventListener("click", () => void openMap());
  els.btnAddMap.addEventListener("click", () => void addMap());
  els.btnProject.addEventListener("click", () => void openProject());
  els.save.addEventListener("click", () => void saveProject(false));
  onFile(els.fileLog, (file) => void ingestFile(file));
  onFile(els.fileMap, (file, mode) => void ingestFile(file, mode));
  onFile(els.fileProject, (file) => void ingestFile(file));

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
