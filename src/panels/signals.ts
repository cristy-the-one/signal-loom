import { els } from "../dom";
import { formatCount, hexId } from "../format";
import { isChecksumSignal, isHousekeepingSignal } from "../lanes";
import { colorFor, hasData } from "../model";
import { refresh, refreshOverview, setPlotted } from "../query";
import { renderDirty } from "../readout";
import { draw, state } from "../state";
import type { SignalInfo, Summary } from "../types";
import { createVirtualList, type VirtualList } from "../virtual-list";
import { clearSignalDropPreview, commitSignalDrop, previewSignalDrop } from "./cluster";
import { openColorPicker } from "./color-picker";

let dragGhost: HTMLDivElement | null = null;
let dragGhostBadge: HTMLSpanElement | null = null;

function ensureDragGhost(signalName: string): HTMLDivElement {
  if (!dragGhost) {
    dragGhost = document.createElement("div");
    dragGhost.className = "sig-drag-ghost";
    dragGhost.hidden = true;
    document.body.append(dragGhost);
  }
  const dot = document.createElement("i");
  dot.style.background = colorFor(signalName);
  const name = document.createElement("span");
  name.className = "sig-drag-name";
  name.textContent = signalName;
  dragGhostBadge = document.createElement("span");
  dragGhostBadge.className = "sig-drag-badge";
  dragGhostBadge.textContent = "Drop on cluster";
  dragGhost.replaceChildren(dot, name, dragGhostBadge);
  dragGhost.classList.remove("is-ready");
  return dragGhost;
}

function startSignalPointerDrag(signalName: string, downEvent: PointerEvent): void {
  const startX = downEvent.clientX;
  const startY = downEvent.clientY;
  let active = false;

  const positionGhost = (x: number, y: number, ready: boolean, action: string) => {
    if (!dragGhost || !dragGhostBadge) return;
    // Offset slightly right/down from the pointer so the cursor never covers the label.
    dragGhost.style.left = `${Math.round(x + 14)}px`;
    dragGhost.style.top = `${Math.round(y + 14)}px`;
    dragGhost.classList.toggle("is-ready", ready);
    dragGhostBadge.textContent = ready ? action : "Drop on cluster";
  };

  const cleanup = (suppressNextClick: boolean) => {
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    window.removeEventListener("pointercancel", onCancel);
    window.removeEventListener("keydown", onKeyDown);
    document.body.classList.remove("is-dragging-signal");
    if (dragGhost) dragGhost.hidden = true;
    clearSignalDropPreview();
    if (suppressNextClick) {
      const stopClick = (e: MouseEvent) => {
        e.preventDefault();
        e.stopPropagation();
      };
      window.addEventListener("click", stopClick, { capture: true, once: true });
      window.setTimeout(() => window.removeEventListener("click", stopClick, true), 50);
    }
  };

  const onMove = (event: PointerEvent) => {
    if (!active) {
      if (Math.hypot(event.clientX - startX, event.clientY - startY) < 6) return;
      active = true;
      ensureDragGhost(signalName).hidden = false;
      document.body.classList.add("is-dragging-signal");
    }
    const { over, action } = previewSignalDrop(event.clientX, event.clientY);
    positionGhost(event.clientX, event.clientY, over, action);
  };

  const onUp = (event: PointerEvent) => {
    if (!active) {
      cleanup(false);
      return;
    }
    commitSignalDrop(signalName, event.clientX, event.clientY);
    cleanup(true);
  };

  const onCancel = () => cleanup(active);

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape") {
      event.preventDefault();
      cleanup(active);
    }
  };

  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
  window.addEventListener("pointercancel", onCancel);
  window.addEventListener("keydown", onKeyDown);
}

/** Must match `.sig { height }` in styles.css. */
const SIG_ROW_HEIGHT = 38;

let sigVirtual: VirtualList<SignalInfo> | null = null;
let cachedSummary: Summary | null = null;
let cachedOrdered: SignalInfo[] = [];
let cachedHaystack: string[] = [];
let cachedHousekeepingCount = 0;
let datalistSummary: Summary | null = null;
let peekTimer = 0;

function ensureOrdered(summary: Summary): { ordered: SignalInfo[]; haystacks: string[]; housekeepingCount: number } {
  if (cachedSummary === summary) {
    return { ordered: cachedOrdered, haystacks: cachedHaystack, housekeepingCount: cachedHousekeepingCount };
  }
  const signals = summary.signals;
  // Physical data signals first, then CRC/counter housekeeping signals, then empty map entries dimmed.
  const active = signals.filter(hasData);
  const primary = active.filter((s) => !isHousekeepingSignal(s.name));
  const housekeeping = active.filter((s) => isHousekeepingSignal(s.name));
  const empty = signals.filter((s) => !hasData(s));
  cachedHousekeepingCount = housekeeping.length;
  cachedOrdered = [...primary, ...housekeeping, ...empty];
  cachedHaystack = cachedOrdered.map((signal) => {
    const hex = signal.messageId == null ? "" : hexId(signal.messageId);
    const ch = signal.channel ? `@${signal.channel} ch${signal.channel}` : "";
    const tag = isChecksumSignal(signal.name) ? "checksum crc chk" : isHousekeepingSignal(signal.name) ? "counter alive" : "";
    return `${signal.name} ${signal.definedName ?? ""} ${signal.unit} ${signal.messageName} ${hex} ${ch} ${tag}`.toLowerCase();
  });
  cachedSummary = summary;
  return { ordered: cachedOrdered, haystacks: cachedHaystack, housekeepingCount: cachedHousekeepingCount };
}

function syncAutocomplete(summary: Summary | null): void {
  if (datalistSummary === summary) return;
  datalistSummary = summary;
  if (!summary) {
    els.signalNames.replaceChildren();
    return;
  }
  const frag = document.createDocumentFragment();
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
  const isChk = isChecksumSignal(signal.name);
  const isHk = isHousekeepingSignal(signal.name);
  row.className = hasData(signal) ? (isHk ? "sig is-hk" : "sig") : "sig is-empty";
  if (!hasData(signal)) {
    row.title = "No frame in this log carries this signal";
  } else {
    row.title = isChk
      ? "Integrity checksum — bad frames already appear on the event timeline · Click to overlay"
      : "Click to plot · Drag onto the cluster bar to add a gadget";
    row.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      if ((event.target as HTMLElement | null)?.closest(".swatch")) return;
      startSignalPointerDrag(signal.name, event);
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
  const swatch = document.createElement("button");
  swatch.type = "button";
  swatch.className = "swatch";
  swatch.style.background = colorFor(signal.name);
  swatch.title = `Change color for ${signal.name}`;
  swatch.setAttribute("aria-label", `Change color for ${signal.name}`);
  swatch.addEventListener("click", (event) => {
    event.preventDefault();
    event.stopPropagation();
    openColorPicker(signal.name, swatch);
  });
  const copy = document.createElement("span");
  copy.className = "sig-copy";
  const name = document.createElement("span");
  name.className = "sig-name";
  name.textContent = signal.name;
  if (isHk) {
    const badge = document.createElement("span");
    badge.className = "sig-badge";
    badge.textContent = isChk ? "CHK" : "CNT";
    name.append(badge);
  }
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
  const swatch = el.querySelector<HTMLElement>(".swatch");
  if (swatch) swatch.style.background = colorFor(signal.name);
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
    els.btnHideChk.hidden = true;
    return;
  }
  const { ordered, haystacks, housekeepingCount } = ensureOrdered(summary);
  els.btnHideChk.hidden = housekeepingCount === 0;
  els.btnHideChk.classList.toggle("is-on", state.hideHousekeeping);
  els.btnHideChk.title = state.hideHousekeeping
    ? `Showing without ${housekeepingCount} checksum/counter signals — click to show`
    : `Hide ${housekeepingCount} checksum/counter signals`;

  const q = state.filter.trim().toLowerCase();
  const visible = ordered.filter((sig, idx) => {
    if (state.hideHousekeeping && isHousekeepingSignal(sig.name) && !state.plotted.includes(sig.name)) {
      return false;
    }
    return !q || haystacks[idx].includes(q);
  });

  els.sigCount.textContent =
    q || state.hideHousekeeping
      ? `${formatCount(visible.length)} / ${formatCount(summary.signals.length)}`
      : formatCount(summary.signals.length);

  const emptyText = summary.signals.length === 0 ? "No decoded signals" : "No signals match";
  virtualList().setItems(visible, emptyText);
}

export function setRailCollapsed(collapsed: boolean, markDirty = true): void {
  const changed = state.railCollapsed !== collapsed;
  state.railCollapsed = collapsed;
  els.workspace.classList.toggle("is-rail-hidden", collapsed);
  if (!collapsed) els.workspace.classList.remove("is-rail-peek");
  els.btnRailToggle.textContent = collapsed ? "▶" : "◀";
  els.btnRailToggle.title = collapsed
    ? "Pin sidebar open (Ctrl+B)"
    : "Hide sidebar — hover left edge to peek (Ctrl+B)";
  els.btnRailToggle.setAttribute("aria-label", collapsed ? "Pin sidebar" : "Hide sidebar");
  if (markDirty && changed) {
    state.dirty = true;
    renderDirty();
  }
  // Redraw canvases once the grid transitions to fill the reclaimed width
  requestAnimationFrame(() => draw());
}

export function toggleRail(): void {
  setRailCollapsed(!state.railCollapsed, true);
}

/** Temporarily peek the collapsed sidebar (e.g. when pressing `/` or hovering the edge). */
export function peekRail(): void {
  if (!state.railCollapsed) return;
  window.clearTimeout(peekTimer);
  els.workspace.classList.add("is-rail-peek");
}

function scheduleHidePeek(): void {
  if (!state.railCollapsed) return;
  window.clearTimeout(peekTimer);
  peekTimer = window.setTimeout(() => {
    if (els.rail.contains(document.activeElement) || !els.colorMenu.hidden) return;
    els.workspace.classList.remove("is-rail-peek");
  }, 150);
}

export function toggleHousekeeping(): void {
  state.hideHousekeeping = !state.hideHousekeeping;
  state.dirty = true;
  renderDirty();
  renderSignals();
}

export function setClusterHidden(hidden: boolean, markDirty = true): void {
  const changed = state.clusterHidden !== hidden;
  state.clusterHidden = hidden;
  els.viewportBody.classList.toggle("is-cluster-hidden", hidden);
  if (markDirty && changed) {
    state.dirty = true;
    renderDirty();
  }
  requestAnimationFrame(() => draw());
}

export function toggleClusterStrip(): void {
  setClusterHidden(!state.clusterHidden, true);
}

export function bindSignals(): void {
  els.sigFilter.addEventListener("input", () => {
    state.filter = els.sigFilter.value;
    renderSignals();
  });
  els.sigFilter.addEventListener("blur", () => scheduleHidePeek());
  els.btnHideChk.addEventListener("click", () => toggleHousekeeping());
  els.btnRailToggle.addEventListener("click", () => toggleRail());

  els.railEdge.addEventListener("pointerenter", () => peekRail());
  els.railEdge.addEventListener("click", () => setRailCollapsed(false));
  els.rail.addEventListener("pointerenter", () => {
    if (state.railCollapsed) peekRail();
  });
  els.rail.addEventListener("pointerleave", () => scheduleHidePeek());
}
