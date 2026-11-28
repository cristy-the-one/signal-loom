import { els } from "../dom";
import { formatCount, hexId } from "../format";
import { colorFor, hasData } from "../model";
import { refresh, refreshOverview, setPlotted } from "../query";
import { renderDirty } from "../readout";
import { draw, state } from "../state";
import type { SignalInfo, Summary } from "../types";
import { createVirtualList, type VirtualList } from "../virtual-list";

/** Must match `.sig { height }` in styles.css. */
const SIG_ROW_HEIGHT = 38;

let sigVirtual: VirtualList<SignalInfo> | null = null;
let cachedSummary: Summary | null = null;
let cachedOrdered: SignalInfo[] = [];
let cachedHaystack: string[] = [];
let datalistSummary: Summary | null = null;

function ensureOrdered(summary: Summary): { ordered: SignalInfo[]; haystacks: string[] } {
  if (cachedSummary === summary) {
    return { ordered: cachedOrdered, haystacks: cachedHaystack };
  }
  const signals = summary.signals;
  // Signals this log carries first; the rest of the map is listed, dimmed.
  cachedOrdered = [...signals.filter(hasData), ...signals.filter((s) => !hasData(s))];
  cachedHaystack = cachedOrdered.map((signal) => {
    const hex = signal.messageId == null ? "" : hexId(signal.messageId);
    const ch = signal.channel ? `@${signal.channel} ch${signal.channel}` : "";
    return `${signal.name} ${signal.definedName ?? ""} ${signal.unit} ${signal.messageName} ${hex} ${ch}`.toLowerCase();
  });
  cachedSummary = summary;
  return { ordered: cachedOrdered, haystacks: cachedHaystack };
}

function syncAutocomplete(summary: Summary | null): void {
  if (datalistSummary === summary) return;
  datalistSummary = summary;
  if (!summary) {
    els.signalNames.replaceChildren();
    return;
  }
  const frag = document.createDocumentFragment();
  // Offer active signals first, capped at 600 options so native `<datalist>` stays instant.
  const active = summary.signals.filter(hasData).slice(0, 600);
  for (const sig of active) {
    const opt = document.createElement("option");
    opt.value = sig.name;
    frag.append(opt);
  }
  els.signalNames.replaceChildren(frag);
}

function buildSignalRow(signal: SignalInfo): HTMLElement {
  const row = document.createElement("label");
  row.className = hasData(signal) ? "sig" : "sig is-empty";
  if (!hasData(signal)) {
    row.title = "No frame in this log carries this signal";
  } else {
    row.draggable = true;
    row.title = "Click to plot · Drag onto the cluster bar to add a gadget";
    row.addEventListener("dragstart", (event) => {
      event.dataTransfer?.setData("text/x-signal", signal.name);
      event.dataTransfer?.setData("text/plain", signal.name);
      if (event.dataTransfer) event.dataTransfer.effectAllowed = "copy";
    });
  }
  const input = document.createElement("input");
  input.type = "checkbox";
  input.checked = state.plotted.includes(signal.name);
  input.addEventListener("change", () => {
    if (input.checked) {
      if (!state.plotted.includes(signal.name)) setPlotted([...state.plotted, signal.name]);
    } else {
      setPlotted(state.plotted.filter((name) => name !== signal.name));
    }
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
  const ch = signal.channel ? ` · ch${signal.channel}` : "";
  meta.textContent = (signal.unit ? `${signal.unit} · ${id}` : id) + ch;
  copy.append(name, meta);
  row.append(input, swatch, copy);
  return row;
}

function syncSignalRow(el: HTMLElement, signal: SignalInfo): void {
  const input = el.querySelector("input");
  if (input) input.checked = state.plotted.includes(signal.name);
}

function virtualList(): VirtualList<SignalInfo> {
  sigVirtual ??= createVirtualList<SignalInfo>(els.sigList, SIG_ROW_HEIGHT, buildSignalRow, syncSignalRow);
  return sigVirtual;
}

export function renderSignals(): void {
  const summary = state.summary;
  syncAutocomplete(summary);
  if (!summary) {
    els.sigList.replaceChildren();
    sigVirtual?.setItems([]);
    return;
  }
  const { ordered, haystacks } = ensureOrdered(summary);
  const q = state.filter.trim().toLowerCase();
  const visible = q
    ? ordered.filter((_, idx) => haystacks[idx].includes(q))
    : ordered;

  els.sigCount.textContent = q
    ? `${formatCount(visible.length)} / ${formatCount(summary.signals.length)}`
    : formatCount(summary.signals.length);

  const emptyText = summary.signals.length === 0 ? "No decoded signals" : "No signals match";
  virtualList().setItems(visible, emptyText);
}

export function bindSignals(): void {
  els.sigFilter.addEventListener("input", () => {
    state.filter = els.sigFilter.value;
    renderSignals();
  });
}
