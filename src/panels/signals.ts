import { els } from "../dom";
import { hexId } from "../format";
import { colorFor, hasData } from "../model";
import { refresh, refreshOverview, setPlotted } from "../query";
import { renderDirty } from "../readout";
import { draw, state } from "../state";

function visibleSignals() {
  const signals = state.summary?.signals ?? [];
  const q = state.filter.trim().toLowerCase();
  if (!q) return signals;
  return signals.filter((signal) => {
    const hay = `${signal.name} ${signal.unit} ${signal.messageName}`.toLowerCase();
    return hay.includes(q);
  });
}

export function renderSignals(): void {
  els.sigList.replaceChildren();
  const signals = visibleSignals();
  if (!state.summary) return;
  if (signals.length === 0) {
    const note = document.createElement("p");
    note.className = "empty-note";
    note.textContent = state.summary.signals.length === 0 ? "No decoded signals" : "No signals match";
    els.sigList.append(note);
    return;
  }
  // Signals this log carries first; the rest of the map is listed, dimmed.
  const ordered = [...signals.filter(hasData), ...signals.filter((signal) => !hasData(signal))];
  for (const signal of ordered) {
    const row = document.createElement("label");
    row.className = hasData(signal) ? "sig" : "sig is-empty";
    if (!hasData(signal)) row.title = "No frame in this log carries this signal";
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = state.plotted.includes(signal.name);
    input.addEventListener("change", () => {
      if (input.checked) setPlotted([...state.plotted, signal.name]);
      else setPlotted(state.plotted.filter((name) => name !== signal.name));
      state.dirty = true;
      renderDirty();
      draw();
      void refresh();
      void refreshOverview();
    });
    const swatch = document.createElement("span");
    swatch.className = "swatch";
    swatch.style.background = colorFor(signal.name);
    const copy = document.createElement("span");
    copy.className = "sig-copy";
    const name = document.createElement("span");
    name.className = "sig-name";
    name.textContent = signal.name;
    const meta = document.createElement("span");
    meta.className = "sig-meta";
    const id = signal.messageId == null ? signal.messageName : `${hexId(signal.messageId)} ${signal.messageName}`;
    meta.textContent = signal.unit ? `${signal.unit} · ${id}` : id;
    copy.append(name, meta);
    row.append(input, swatch, copy);
    els.sigList.append(row);
  }
}

export function bindSignals(): void {
  els.sigFilter.addEventListener("input", () => {
    state.filter = els.sigFilter.value;
    renderSignals();
  });
}
