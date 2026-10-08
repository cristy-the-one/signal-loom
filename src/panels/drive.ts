import * as api from "../api";
import { els } from "../dom";
import { LOG_FILTERS, download, onFile, pickPath, pickSavePath, withExtension } from "../files";
import { basename, formatBytes, formatCount } from "../format";
import { windowFor } from "../model";
import { adoptEdited, adoptSummary } from "../project";
import { cancelCursorStats, refreshCursors } from "../query";
import { draw, state } from "../state";
import { indexThen, serialDeck, setError, setNotice, withBusy } from "../status";

export function renderCompare(): void {
  if (!state.compareOn) {
    els.compareLabel.textContent = "No second drive";
    return;
  }
  const name = state.comparePath ? basename(state.comparePath) : "uploaded log";
  els.compareLabel.textContent = `${name} · offset ${state.compareOffsetUs / 1000} ms`;
}

async function openCompare(): Promise<void> {
  const path = await pickPath(els.fileCompare, LOG_FILTERS);
  if (!path) return;
  await serialDeck(() =>
    indexThen(`Comparing ${basename(path)}`, () => api.beginCompare(path), (summary) => {
      state.compareOn = true;
      state.comparePath = path;
      adoptEdited(summary);
    }),
  );
}

async function ingestCompareFile(file: File): Promise<void> {
  await serialDeck(() =>
    withBusy(`Comparing ${file.name}`, async () => {
      const summary = await api.openCompareBytes(await file.arrayBuffer());
      state.compareOn = true;
      state.comparePath = file.name;
      adoptEdited(summary);
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
      adoptEdited(summary);
    }),
  );
}

async function applyOffset(): Promise<void> {
  const ms = Number(els.compareOffset.value);
  if (!Number.isFinite(ms)) return;
  const offsetUs = Math.round(ms * 1000);
  if (!state.summary) {
    state.compareOffsetUs = offsetUs;
    state.dirty = true;
    renderCompare();
    return;
  }
  await serialDeck(async () => {
    await withBusy("Aligning drives", async () => {
      const summary = await api.setCompareOffset(offsetUs);
      state.compareOffsetUs = offsetUs;
      adoptEdited(summary);
    });
    // A refused offset leaves the field showing the offset in effect.
    els.compareOffset.value = String(state.compareOffsetUs / 1000);
  });
}

export function dropCursor(which: "a" | "b"): void {
  if (!state.summary) return;
  const t = Math.round(state.playhead);
  if (which === "a") state.cursorA = t;
  else state.cursorB = t;
  state.dirty = true;
  void refreshCursors();
  draw();
}

function clearCursors(): void {
  state.cursorA = null;
  state.cursorB = null;
  state.cursorText = "";
  cancelCursorStats();
  state.dirty = true;
  draw();
}

/** The span between the cursors when both are down, else the visible window. */
function exportWindow(): { t0: number; t1: number } | null {
  if (!state.summary) return null;
  if (state.cursorA != null && state.cursorB != null) {
    return { t0: Math.min(state.cursorA, state.cursorB), t1: Math.max(state.cursorA, state.cursorB) };
  }
  const view = windowFor(state.playhead, state.span, state.summary);
  return { t0: view.t0, t1: view.t1 };
}

function truncatedNotice(rows: number): string {
  return `Export stopped at ${formatCount(rows)} rows. Narrow the window for the rest.`;
}

async function exportRange(kind: "csv" | "slog"): Promise<void> {
  const range = exportWindow();
  if (!range || !state.summary) {
    setError("Open a log before exporting.", "action");
    return;
  }
  const names = state.plotted.length ? state.plotted : state.summary.signals.slice(0, 1).map((signal) => signal.name);
  const suggested = `signal-loom-${Math.round(range.t0)}-${Math.round(range.t1)}.${kind}`;
  const label = kind === "csv" ? "Exporting CSV" : "Trimming log";
  if (api.inTauri()) {
    // The desktop webview has no download UI: ask where, and let Rust write it.
    const picked = await pickSavePath(
      suggested,
      kind === "csv" ? { name: "CSV", extensions: ["csv"] } : { name: "Signal Loom log", extensions: ["slog"] },
    );
    if (!picked) return;
    const target = withExtension(picked, `.${kind}`);
    await withBusy(label, async () => {
      const saved =
        kind === "csv"
          ? await api.saveCsv(target, names, range.t0, range.t1)
          : await api.saveSlog(target, range.t0, range.t1);
      const message = `Saved ${basename(target)} (${formatBytes(saved.bytes)})`;
      setNotice(saved.truncated ? `${message}. ${truncatedNotice(saved.rows)}` : message);
    });
    return;
  }
  await withBusy(label, async () => {
    if (kind === "csv") {
      const csv = await api.exportCsv(names, range.t0, range.t1);
      download(suggested, csv.text, "text/csv");
      if (csv.truncated) setNotice(truncatedNotice(csv.rows));
    } else {
      const slog = await api.exportSlog(range.t0, range.t1);
      download(suggested, slog.text, "text/plain");
      if (slog.truncated) setNotice(truncatedNotice(slog.rows));
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
  await indexThen(`Listening on ${iface}`, () => api.beginCapture(iface, durationMs), (summary) =>
    adoptSummary(summary, "fresh"),
  );
}

export function bindDrive(): void {
  els.btnCompare.addEventListener("click", () => void openCompare());
  els.btnCompareClear.addEventListener("click", () => void clearCompareDrive());
  els.compareOffset.addEventListener("change", () => void applyOffset());
  onFile(els.fileCompare, (file) => void ingestCompareFile(file));
  els.cursorA.addEventListener("click", () => dropCursor("a"));
  els.cursorB.addEventListener("click", () => dropCursor("b"));
  els.cursorClear.addEventListener("click", clearCursors);
  els.exportCsv.addEventListener("click", () => void exportRange("csv"));
  els.exportSlog.addEventListener("click", () => void exportRange("slog"));
  els.captureArm.addEventListener("change", () => {
    els.captureBtn.disabled = !els.captureArm.checked;
  });
  els.captureBtn.addEventListener("click", () => void captureBus());
}
