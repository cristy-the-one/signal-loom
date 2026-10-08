import { els } from "./dom";
import { formatBytes, formatCount } from "./format";
import { renderMath, renderTriggers } from "./panels/deck";
import { renderCompare } from "./panels/drive";
import { renderEvents } from "./panels/events";
import { renderMarks, renderNotes } from "./panels/marks";
import { renderSignals } from "./panels/signals";
import { renderDirty, renderTransport } from "./readout";
import { state } from "./state";

/** Rebuilds the header, import warnings and every rail list from the state. */
export function paintChrome(): void {
  const summary = state.summary;
  renderDirty();
  document.title = summary ? `Signal Loom — ${summary.logLabel}` : "Signal Loom";
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
