import { els } from "../dom";
import {
  LAMP_OPS,
  WIDGET_COLORS,
  WIDGET_KINDS,
  WIDGET_KIND_LABELS,
  defaultClusterWidgets,
  dropIndexAt,
  layoutWidgets,
  resolveWidgets,
  smartWidgetForSignal,
  widgetAt,
  type ClusterInteraction,
  type ResolvedWidget,
} from "../gauges";
import { hasData, signalNamed } from "../model";
import { draw, renderChrome, state } from "../state";
import { setNotice } from "../status";
import type { ClusterWidget, LampOp, WidgetKind } from "../types";

const MAX_WIDGETS = 32;
/** Idle timeout before an unaccepted add/modify gadget popover auto-cancels. */
const EDITOR_IDLE_MS = 15_000;

let hoverIndex = -1;
let editingId: string | null = null;
let editingIsNew = false;
let editSnapshot: { widgets: ClusterWidget[]; customized: boolean; dirty: boolean } | null = null;
let editorIdleTimer = 0;
let editorTickTimer = 0;
let editorDeadline = 0;
let dragIndex = -1;
let dropIndex: number | null = null;
let scrollX = 0;

function clearEditorTimeout(): void {
  window.clearTimeout(editorIdleTimer);
  window.clearInterval(editorTickTimer);
  editorIdleTimer = 0;
  editorTickTimer = 0;
  editorDeadline = 0;
  els.widgetTimeoutText.hidden = true;
  els.widgetTimeoutText.textContent = "";
  els.widgetTimeoutBar.style.transition = "none";
  els.widgetTimeoutBar.style.width = "100%";
}

function resetEditorTimeout(): void {
  if (!editingId || els.widgetEditor.hidden) return;
  clearEditorTimeout();
  editorDeadline = Date.now() + EDITOR_IDLE_MS;
  // Restart smooth linear progress bar countdown
  els.widgetTimeoutBar.style.transition = "none";
  els.widgetTimeoutBar.style.width = "100%";
  void els.widgetTimeoutBar.offsetWidth;
  els.widgetTimeoutBar.style.transition = `width ${EDITOR_IDLE_MS}ms linear`;
  els.widgetTimeoutBar.style.width = "0%";

  editorTickTimer = window.setInterval(() => {
    const leftMs = editorDeadline - Date.now();
    if (leftMs <= 5_000 && leftMs > 0) {
      els.widgetTimeoutText.hidden = false;
      els.widgetTimeoutText.textContent = `${Math.ceil(leftMs / 1000)}s`;
    } else {
      els.widgetTimeoutText.hidden = true;
    }
  }, 250);

  editorIdleTimer = window.setTimeout(() => {
    const msg = editingIsNew
      ? "Timed out — cancelled adding gadget."
      : "Timed out — reverted gadget changes.";
    cancelWidgetEditor(msg);
  }, EDITOR_IDLE_MS);
}

/** Current interactive overlay state for `drawWidgets` in `plot-view.ts`. */
export function clusterInteraction(): ClusterInteraction {
  const selectedIndex = editingId ? state.clusterWidgets.findIndex((w) => w.id === editingId) : -1;
  return { hoverIndex, selectedIndex, dragIndex, dropIndex, scrollX };
}

/** Update the cluster strip drop preview while pointer-dragging a signal from the Signals rail. */
export function previewSignalDrop(clientX: number, clientY: number): { over: boolean; action: string } {
  if (!state.summary || state.clusterHidden) {
    clearSignalDropPreview();
    return { over: false, action: "" };
  }
  const rect = els.gauges.getBoundingClientRect();
  const over =
    rect.width > 0 &&
    rect.height > 0 &&
    clientX >= rect.left &&
    clientX <= rect.right &&
    clientY >= rect.top &&
    clientY <= rect.bottom;
  if (!over) {
    clearSignalDropPreview();
    return { over: false, action: "" };
  }
  const x = clientX - rect.left;
  const y = clientY - rect.top;
  const resolved = currentResolved();
  const hit = widgetAt(x, y, rect.width, rect.height, resolved, scrollX);
  let nextHover = -1;
  let nextDrop: number | null = null;
  let action = "+ Add gadget";
  if (hit >= 0) {
    nextHover = hit;
    action = `Bind to ${resolved[hit]?.label || "gadget"}`;
  } else {
    nextDrop = dropIndexAt(x, resolved, rect.height, scrollX);
  }
  if (nextHover !== hoverIndex || nextDrop !== dropIndex) {
    hoverIndex = nextHover;
    dropIndex = nextDrop;
    draw();
  }
  return { over: true, action };
}

/** Commit a pointer-dragged signal onto the cluster strip if released over it, opening the editor to accept or cancel. */
export function commitSignalDrop(sigName: string, clientX: number, clientY: number): boolean {
  const { over } = previewSignalDrop(clientX, clientY);
  const targetHit = hoverIndex;
  const targetDrop = dropIndex;
  hoverIndex = -1;
  dropIndex = null;
  if (!over || !state.summary) {
    draw();
    return false;
  }
  if (editingId) cancelWidgetEditor();
  const rect = els.gauges.getBoundingClientRect();
  const snapshot = {
    widgets: state.clusterWidgets.map((w) => ({ ...w })),
    customized: state.clusterCustomized,
    dirty: state.dirty,
  };
  if (targetHit >= 0) {
    const next = state.clusterWidgets.map((w, i) => (i === targetHit ? { ...w, signal: sigName } : w));
    commitWidgets(next);
    openWidgetEditor(targetHit, clientX, rect.bottom, { isNew: false, snapshot });
    return true;
  }
  if (state.clusterWidgets.length >= MAX_WIDGETS) {
    draw();
    setNotice(`Cluster holds at most ${MAX_WIDGETS} gadgets.`);
    return false;
  }
  const info = signalNamed(sigName);
  const widget: ClusterWidget = info
    ? smartWidgetForSignal(info, newWidgetId())
    : { id: newWidgetId(), kind: "value", signal: sigName };
  const insertAt = Math.max(0, Math.min(state.clusterWidgets.length, targetDrop ?? state.clusterWidgets.length));
  const next = [...state.clusterWidgets];
  next.splice(insertAt, 0, widget);
  commitWidgets(next);
  openWidgetEditor(insertAt, clientX, rect.bottom, { isNew: true, snapshot });
  return true;
}

export function clearSignalDropPreview(): void {
  if (hoverIndex !== -1 || dropIndex !== null) {
    hoverIndex = -1;
    dropIndex = null;
    draw();
  }
}

function currentResolved(): ResolvedWidget[] {
  if (!state.summary) return [];
  return resolveWidgets(state.clusterWidgets, state.held, state.summary.signals);
}

function clampScroll(resolved: ResolvedWidget[]): void {
  const rect = els.gauges.getBoundingClientRect();
  const { totalWidth } = layoutWidgets(resolved, rect.height || 72, 0);
  const maxScroll = Math.max(0, totalWidth - Math.max(100, rect.width - 110));
  scrollX = Math.max(0, Math.min(scrollX, maxScroll));
}

function newWidgetId(): string {
  return `w-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
}

function commitWidgets(next: ClusterWidget[]): void {
  state.clusterWidgets = next;
  state.clusterCustomized = true;
  state.dirty = true;
  renderChrome();
  draw();
}

export function closeWidgetEditor(): void {
  clearEditorTimeout();
  editingId = null;
  editingIsNew = false;
  editSnapshot = null;
  els.widgetEditor.hidden = true;
  draw();
}

/** Cancel the current edit session (Esc, outside click, ×, or idle timeout): removes a brand-new draft, or reverts edits. */
export function cancelWidgetEditor(notice?: string): void {
  if (!editingId) return;
  clearEditorTimeout();
  const snap = editSnapshot;
  const wasNew = editingIsNew;
  editingId = null;
  editingIsNew = false;
  editSnapshot = null;
  els.widgetEditor.hidden = true;
  if (snap) {
    state.clusterWidgets = snap.widgets.map((w) => ({ ...w }));
    state.clusterCustomized = snap.customized;
    state.dirty = snap.dirty;
    renderChrome();
  }
  draw();
  if (notice) setNotice(notice);
  else if (wasNew) setNotice("Cancelled adding gadget.");
}

function updateEditingWidget(patch: Partial<ClusterWidget>): void {
  if (!editingId) return;
  const idx = state.clusterWidgets.findIndex((w) => w.id === editingId);
  if (idx < 0) return;
  const next = state.clusterWidgets.map((w, i) => (i === idx ? { ...w, ...patch } : w));
  commitWidgets(next);
}

function parseOptionalNumber(raw: string): number | undefined {
  const t = raw.trim();
  if (!t) return undefined;
  const n = Number(t);
  return Number.isFinite(n) ? n : undefined;
}

function renderSignalPicker(currentSignal: string | null, filter: string): void {
  const host = els.widgetSigList;
  host.replaceChildren();
  const q = filter.trim().toLowerCase();

  const addOption = (value: string | null, title: string, meta: string, active: boolean) => {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = active ? "picker-item is-active" : "picker-item";
    const nameEl = document.createElement("span");
    nameEl.className = "picker-name";
    nameEl.textContent = title;
    const metaEl = document.createElement("span");
    metaEl.className = "picker-meta";
    metaEl.textContent = meta;
    btn.append(nameEl, metaEl);
    btn.addEventListener("click", () => {
      updateEditingWidget({ signal: value });
      const edited = state.clusterWidgets.find((w) => w.id === editingId);
      if (edited) syncEditorFields(edited);
    });
    host.append(btn);
  };

  if (!q) {
    addOption(null, "(Unbound / Default)", "auto", !currentSignal);
  }

  const signals = (state.summary?.signals ?? [])
    .filter(hasData)
    .sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));

  let shown = 0;
  for (const sig of signals) {
    const hay = `${sig.name} ${sig.unit} ${sig.messageName}`.toLowerCase();
    if (q && !hay.includes(q)) continue;
    const meta = [sig.unit, sig.messageName].filter(Boolean).join(" · ");
    addOption(sig.name, sig.name, meta, currentSignal === sig.name);
    shown += 1;
    if (shown >= 80) break;
  }
  if (shown === 0 && q) {
    const empty = document.createElement("div");
    empty.className = "picker-empty";
    empty.textContent = "No matching signals";
    host.append(empty);
  }
}

function syncEditorFields(widget: ClusterWidget): void {
  const idx = state.clusterWidgets.findIndex((w) => w.id === widget.id);
  const resolved = currentResolved()[idx];

  els.widgetMoveLeft.disabled = idx <= 0;
  els.widgetMoveRight.disabled = idx < 0 || idx >= state.clusterWidgets.length - 1;
  els.widgetDup.disabled = state.clusterWidgets.length >= MAX_WIDGETS;

  // Kind buttons
  els.widgetKindRow.replaceChildren(
    ...WIDGET_KINDS.map((kind) => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = widget.kind === kind ? "seg-btn is-on" : "seg-btn";
      btn.textContent = WIDGET_KIND_LABELS[kind];
      btn.addEventListener("click", () => {
        updateEditingWidget({ kind });
        const updated = state.clusterWidgets.find((w) => w.id === editingId);
        if (updated) syncEditorFields(updated);
      });
      return btn;
    }),
  );

  // Label
  els.widgetLabel.value = widget.label ?? "";
  els.widgetLabel.placeholder = resolved?.autoLabel || "Auto";

  // Color swatches + custom color input
  const activeColor = widget.color ?? resolved?.color ?? WIDGET_COLORS[0].hex;
  const presetMatch = WIDGET_COLORS.some((c) => c.hex.toLowerCase() === activeColor.toLowerCase());
  const customWrap = document.createElement("label");
  customWrap.className = presetMatch ? "color-custom-wrap" : "color-custom-wrap is-on";
  customWrap.title = "Pick any custom color";
  const customInput = document.createElement("input");
  customInput.type = "color";
  customInput.className = "color-custom-input";
  customInput.value = /^#[0-9a-f]{6}$/i.test(activeColor) ? activeColor : "#3ec6ff";
  customInput.setAttribute("aria-label", "Custom gadget color");
  customInput.addEventListener("input", () => {
    updateEditingWidget({ color: customInput.value.toLowerCase() });
    customWrap.classList.add("is-on");
    customWrap.style.background = customInput.value;
    els.widgetColors.querySelectorAll(".color-dot.is-on").forEach((el) => el.classList.remove("is-on"));
  });
  if (!presetMatch) customWrap.style.background = activeColor;
  customWrap.append(customInput);

  els.widgetColors.replaceChildren(
    ...WIDGET_COLORS.map((c) => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = activeColor.toLowerCase() === c.hex.toLowerCase() ? "color-dot is-on" : "color-dot";
      btn.style.background = c.hex;
      btn.title = c.name;
      btn.setAttribute("aria-label", c.name);
      btn.addEventListener("click", () => {
        updateEditingWidget({ color: c.hex });
        const updated = state.clusterWidgets.find((w) => w.id === editingId);
        if (updated) syncEditorFields(updated);
      });
      return btn;
    }),
    customWrap,
  );

  // Kind-specific fields
  const isScaleKind = widget.kind === "arc" || widget.kind === "bar";
  const isLamp = widget.kind === "lamp";
  els.widgetScaleRow.hidden = !isScaleKind;
  els.widgetLampRow.hidden = !isLamp;
  els.widgetDecRow.hidden = isLamp;

  els.widgetMin.value = widget.min != null ? String(widget.min) : "";
  els.widgetMin.placeholder = resolved ? String(resolved.min) : "0";
  els.widgetMax.value = widget.max != null ? String(widget.max) : "";
  els.widgetMax.placeholder = resolved ? String(resolved.max) : "100";
  els.widgetWarn.value = widget.warnFrom != null ? String(widget.warnFrom) : "";
  els.widgetDec.value = widget.decimals != null ? String(widget.decimals) : "";
  els.widgetDec.placeholder = resolved ? String(resolved.decimals) : "auto";

  els.widgetLampOp.value = widget.lampOp ?? ">";
  els.widgetLampVal.value = String(widget.lampThreshold ?? 0.5);

  renderSignalPicker(widget.signal ?? null, els.widgetSigSearch.value);
}

function openWidgetEditor(
  index: number,
  anchorX: number,
  anchorY: number,
  draft?: { isNew: boolean; snapshot: { widgets: ClusterWidget[]; customized: boolean; dirty: boolean } },
): void {
  const widget = state.clusterWidgets[index];
  if (!widget || !state.summary) return;
  editingId = widget.id;
  editingIsNew = draft?.isNew ?? false;
  editSnapshot = draft?.snapshot ?? {
    widgets: state.clusterWidgets.map((w) => ({ ...w })),
    customized: state.clusterCustomized,
    dirty: state.dirty,
  };
  els.widgetHint.innerHTML = editingIsNew
    ? "<kbd>Esc</kbd> cancel"
    : "<kbd>Esc</kbd> revert · <kbd>Del</kbd> remove";
  els.widgetSigSearch.value = "";
  syncEditorFields(widget);

  const editorW = 286;
  const editorH = 392;
  const left = Math.max(8, Math.min(anchorX - 40, window.innerWidth - editorW - 10));
  const top = Math.max(44, Math.min(anchorY + 12, window.innerHeight - editorH - 10));
  els.widgetEditor.style.left = `${left}px`;
  els.widgetEditor.style.top = `${top}px`;
  els.widgetEditor.hidden = false;
  resetEditorTimeout();
  draw();
  els.widgetSigSearch.focus();
}

export function addNewWidget(kind: WidgetKind = "value"): void {
  if (!state.summary) return;
  if (editingId) cancelWidgetEditor();
  if (state.clusterHidden) {
    state.clusterHidden = false;
    els.viewportBody.classList.remove("is-cluster-hidden");
  }
  if (state.clusterWidgets.length >= MAX_WIDGETS) {
    setNotice(`Cluster holds at most ${MAX_WIDGETS} gadgets.`);
    return;
  }
  const snapshot = {
    widgets: state.clusterWidgets.map((w) => ({ ...w })),
    customized: state.clusterCustomized,
    dirty: state.dirty,
  };
  const firstSignal = state.plotted[0] ?? state.summary.signals.find(hasData)?.name ?? null;
  const widget: ClusterWidget = {
    id: newWidgetId(),
    kind,
    signal: firstSignal,
    color: WIDGET_COLORS[state.clusterWidgets.length % WIDGET_COLORS.length].hex,
  };
  const next = [...state.clusterWidgets, widget];
  commitWidgets(next);

  // Scroll to reveal the new widget and open its editor
  const resolved = currentResolved();
  const rect = els.gauges.getBoundingClientRect();
  const { items, totalWidth } = layoutWidgets(resolved, rect.height || 72, 0);
  const maxScroll = Math.max(0, totalWidth - Math.max(100, rect.width - 110));
  scrollX = maxScroll;
  draw();

  const lastBox = items[items.length - 1]?.box;
  const anchorX = lastBox ? rect.left + lastBox.x - scrollX : rect.right - 260;
  openWidgetEditor(next.length - 1, anchorX, rect.bottom, { isNew: true, snapshot });
}

function moveEditingWidget(delta: -1 | 1): void {
  if (!editingId) return;
  const idx = state.clusterWidgets.findIndex((w) => w.id === editingId);
  const target = idx + delta;
  if (idx < 0 || target < 0 || target >= state.clusterWidgets.length) return;
  const next = [...state.clusterWidgets];
  const [moved] = next.splice(idx, 1);
  next.splice(target, 0, moved);
  commitWidgets(next);
  syncEditorFields(moved);
}

function duplicateEditingWidget(): void {
  if (!editingId || state.clusterWidgets.length >= MAX_WIDGETS) return;
  const idx = state.clusterWidgets.findIndex((w) => w.id === editingId);
  if (idx < 0) return;
  const copy: ClusterWidget = { ...state.clusterWidgets[idx], id: newWidgetId() };
  const next = [...state.clusterWidgets];
  next.splice(idx + 1, 0, copy);
  editingId = copy.id;
  commitWidgets(next);
  syncEditorFields(copy);
}

function removeEditingWidget(): void {
  if (!editingId) return;
  clearEditorTimeout();
  const next = state.clusterWidgets.filter((w) => w.id !== editingId);
  editingId = null;
  editingIsNew = false;
  editSnapshot = null;
  els.widgetEditor.hidden = true;
  commitWidgets(next);
}

function resetCluster(): void {
  closeWidgetEditor();
  scrollX = 0;
  state.cluster = {};
  state.clusterWidgets = defaultClusterWidgets();
  state.clusterCustomized = false;
  state.dirty = true;
  renderChrome();
  draw();
  setNotice("Cluster restored to default layout.");
}

export function bindCluster(): void {
  // Populate lamp operators once
  els.widgetLampOp.replaceChildren(
    ...LAMP_OPS.map((op) => {
      const opt = document.createElement("option");
      opt.value = op;
      opt.textContent = op;
      return opt;
    }),
  );

  els.btnClusterAdd.addEventListener("click", () => addNewWidget("value"));
  els.btnClusterReset.addEventListener("click", () => resetCluster());
  els.widgetClose.addEventListener("click", () => cancelWidgetEditor());
  els.widgetAccept.addEventListener("click", () => closeWidgetEditor());
  els.widgetMoveLeft.addEventListener("click", () => moveEditingWidget(-1));
  els.widgetMoveRight.addEventListener("click", () => moveEditingWidget(1));
  els.widgetDup.addEventListener("click", () => duplicateEditingWidget());

  // Any interaction inside the gadget editor resets the inactivity auto-cancel timeout.
  for (const ev of ["pointermove", "pointerdown", "keydown", "input", "change", "wheel", "focusin"] as const) {
    els.widgetEditor.addEventListener(ev, () => resetEditorTimeout(), { passive: true });
  }

  els.widgetSigSearch.addEventListener("input", () => {
    const widget = state.clusterWidgets.find((w) => w.id === editingId);
    if (widget) renderSignalPicker(widget.signal ?? null, els.widgetSigSearch.value);
  });

  els.widgetLabel.addEventListener("input", () => {
    const val = els.widgetLabel.value.trim();
    updateEditingWidget({ label: val || undefined });
  });

  els.widgetMin.addEventListener("input", () => {
    updateEditingWidget({ min: parseOptionalNumber(els.widgetMin.value) });
  });
  els.widgetMax.addEventListener("input", () => {
    updateEditingWidget({ max: parseOptionalNumber(els.widgetMax.value) });
  });
  els.widgetWarn.addEventListener("input", () => {
    updateEditingWidget({ warnFrom: parseOptionalNumber(els.widgetWarn.value) });
  });
  els.widgetDec.addEventListener("input", () => {
    const n = parseOptionalNumber(els.widgetDec.value);
    updateEditingWidget({ decimals: n == null ? undefined : Math.max(0, Math.min(6, Math.round(n))) });
  });
  els.widgetLampOp.addEventListener("change", () => {
    updateEditingWidget({ lampOp: els.widgetLampOp.value as LampOp });
  });
  els.widgetLampVal.addEventListener("input", () => {
    const n = parseOptionalNumber(els.widgetLampVal.value);
    if (n != null) updateEditingWidget({ lampThreshold: n });
  });

  // Pointer drag-to-reorder & click-to-edit on the cluster canvas
  let downIdx = -1;
  let downX = 0;
  let dragging = false;

  els.gauges.addEventListener("pointerdown", (event) => {
    if (event.button !== 0 || !state.summary) return;
    if (editingId) {
      cancelWidgetEditor();
      return;
    }
    const rect = els.gauges.getBoundingClientRect();
    const resolved = currentResolved();
    const idx = widgetAt(event.clientX - rect.left, event.clientY - rect.top, rect.width, rect.height, resolved, scrollX);
    if (idx < 0) return;
    downIdx = idx;
    downX = event.clientX;
    dragging = false;
    els.gauges.setPointerCapture(event.pointerId);
  });

  els.gauges.addEventListener("pointermove", (event) => {
    if (!state.summary) return;
    const rect = els.gauges.getBoundingClientRect();
    const x = event.clientX - rect.left;
    const y = event.clientY - rect.top;
    const resolved = currentResolved();

    if (downIdx >= 0) {
      if (!dragging && Math.abs(event.clientX - downX) > 6) {
        dragging = true;
        dragIndex = downIdx;
        if (editingId) closeWidgetEditor();
      }
      if (dragging) {
        dropIndex = dropIndexAt(x, resolved, rect.height, scrollX);
        els.gauges.classList.add("is-dragging");
        draw();
        return;
      }
    }

    const nextHover = widgetAt(x, y, rect.width, rect.height, resolved, scrollX);
    els.gauges.classList.toggle("on-slot", nextHover >= 0);
    if (nextHover !== hoverIndex) {
      hoverIndex = nextHover;
      draw();
    }
  });

  els.gauges.addEventListener("pointerup", (event) => {
    if (downIdx < 0) return;
    const rect = els.gauges.getBoundingClientRect();
    if (dragging && dragIndex >= 0 && dropIndex != null) {
      const next = [...state.clusterWidgets];
      const [moved] = next.splice(dragIndex, 1);
      const insertAt = dropIndex > dragIndex ? dropIndex - 1 : dropIndex;
      next.splice(Math.max(0, Math.min(next.length, insertAt)), 0, moved);
      downIdx = -1;
      dragIndex = -1;
      dropIndex = null;
      dragging = false;
      els.gauges.classList.remove("is-dragging");
      commitWidgets(next);
      return;
    }
    const clickedIdx = downIdx;
    downIdx = -1;
    dragIndex = -1;
    dropIndex = null;
    dragging = false;
    els.gauges.classList.remove("is-dragging");
    openWidgetEditor(clickedIdx, event.clientX, rect.bottom);
  });

  els.gauges.addEventListener("pointerleave", () => {
    if (!dragging && hoverIndex !== -1) {
      hoverIndex = -1;
      els.gauges.classList.remove("on-slot");
      draw();
    }
  });

  els.gauges.addEventListener("pointercancel", () => {
    downIdx = -1;
    dragIndex = -1;
    dropIndex = null;
    dragging = false;
    els.gauges.classList.remove("is-dragging");
    draw();
  });

  // Horizontal scroll on the cluster strip when there are many widgets
  els.gauges.addEventListener(
    "wheel",
    (event) => {
      if (!state.summary) return;
      const resolved = currentResolved();
      const rect = els.gauges.getBoundingClientRect();
      const { totalWidth } = layoutWidgets(resolved, rect.height || 72, 0);
      if (totalWidth <= rect.width - 110) return;
      event.preventDefault();
      scrollX += Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
      clampScroll(resolved);
      draw();
    },
    { passive: false },
  );

  // HTML5 drag fallback (browser environments) — desktop uses pointer-drag from signals.ts
  els.gauges.addEventListener("dragenter", (event) => {
    if (!state.summary) return;
    if (!event.dataTransfer?.types.includes("text/x-signal")) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
  });

  els.gauges.addEventListener("dragover", (event) => {
    if (!state.summary) return;
    if (!event.dataTransfer?.types.includes("text/x-signal")) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
    previewSignalDrop(event.clientX, event.clientY);
  });

  els.gauges.addEventListener("dragleave", () => {
    clearSignalDropPreview();
  });

  els.gauges.addEventListener("drop", (event) => {
    const sigName = event.dataTransfer?.getData("text/x-signal");
    if (!sigName || !state.summary) {
      clearSignalDropPreview();
      return;
    }
    event.preventDefault();
    commitSignalDrop(sigName, event.clientX, event.clientY);
  });

  // Clicking outside the gadget editor assumes Cancel (discard new draft / revert changes).
  document.addEventListener("pointerdown", (event) => {
    if (
      !els.widgetEditor.hidden &&
      !els.widgetEditor.contains(event.target as Node) &&
      event.target !== els.gauges &&
      event.target !== els.btnClusterAdd
    ) {
      cancelWidgetEditor();
    }
  });

  document.addEventListener("keydown", (event) => {
    if (els.widgetEditor.hidden) return;
    if (event.key === "Enter") {
      const target = event.target as HTMLElement | null;
      if (
        target === els.widgetMoveLeft ||
        target === els.widgetMoveRight ||
        target === els.widgetDup
      ) {
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      closeWidgetEditor();
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      cancelWidgetEditor();
      return;
    }
    if (event.key === "Delete") {
      const target = event.target as HTMLElement | null;
      const editingText =
        (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) &&
        target.value.length > 0;
      if (editingText) return;
      event.preventDefault();
      event.stopPropagation();
      removeEditingWidget();
    }
  });
}
