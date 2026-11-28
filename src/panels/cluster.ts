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

let hoverIndex = -1;
let editingId: string | null = null;
let dragIndex = -1;
let dropIndex: number | null = null;
let scrollX = 0;

/** Current interactive overlay state for `drawWidgets` in `plot-view.ts`. */
export function clusterInteraction(): ClusterInteraction {
  const selectedIndex = editingId ? state.clusterWidgets.findIndex((w) => w.id === editingId) : -1;
  return { hoverIndex, selectedIndex, dragIndex, dropIndex, scrollX };
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
  editingId = null;
  els.widgetEditor.hidden = true;
  draw();
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

  // Color swatches
  const activeColor = widget.color ?? resolved?.color ?? WIDGET_COLORS[0].hex;
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

function openWidgetEditor(index: number, anchorX: number, anchorY: number): void {
  const widget = state.clusterWidgets[index];
  if (!widget || !state.summary) return;
  editingId = widget.id;
  els.widgetSigSearch.value = "";
  syncEditorFields(widget);

  const editorW = 286;
  const editorH = 380;
  const left = Math.max(8, Math.min(anchorX - 40, window.innerWidth - editorW - 10));
  const top = Math.max(44, Math.min(anchorY + 12, window.innerHeight - editorH - 10));
  els.widgetEditor.style.left = `${left}px`;
  els.widgetEditor.style.top = `${top}px`;
  els.widgetEditor.hidden = false;
  draw();
  els.widgetSigSearch.focus();
}

function addNewWidget(kind: WidgetKind = "value"): void {
  if (!state.summary) return;
  if (state.clusterWidgets.length >= MAX_WIDGETS) {
    setNotice(`Cluster holds at most ${MAX_WIDGETS} gadgets.`);
    return;
  }
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
  openWidgetEditor(next.length - 1, anchorX, rect.bottom);
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
  const next = state.clusterWidgets.filter((w) => w.id !== editingId);
  editingId = null;
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
  els.widgetClose.addEventListener("click", () => closeWidgetEditor());
  els.widgetMoveLeft.addEventListener("click", () => moveEditingWidget(-1));
  els.widgetMoveRight.addEventListener("click", () => moveEditingWidget(1));
  els.widgetDup.addEventListener("click", () => duplicateEditingWidget());
  els.widgetRemove.addEventListener("click", () => removeEditingWidget());

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

  // Drag-and-drop signals from the Signals rail directly onto the cluster strip!
  els.gauges.addEventListener("dragover", (event) => {
    if (!state.summary) return;
    if (!event.dataTransfer?.types.includes("text/x-signal")) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
    const rect = els.gauges.getBoundingClientRect();
    const x = event.clientX - rect.left;
    const y = event.clientY - rect.top;
    const resolved = currentResolved();
    const hit = widgetAt(x, y, rect.width, rect.height, resolved, scrollX);
    if (hit >= 0) {
      hoverIndex = hit;
      dropIndex = null;
    } else {
      hoverIndex = -1;
      dropIndex = dropIndexAt(x, resolved, rect.height, scrollX);
    }
    draw();
  });

  els.gauges.addEventListener("dragleave", () => {
    hoverIndex = -1;
    dropIndex = null;
    draw();
  });

  els.gauges.addEventListener("drop", (event) => {
    const sigName = event.dataTransfer?.getData("text/x-signal");
    hoverIndex = -1;
    const targetDrop = dropIndex;
    dropIndex = null;
    if (!sigName || !state.summary) return;
    event.preventDefault();
    const rect = els.gauges.getBoundingClientRect();
    const x = event.clientX - rect.left;
    const y = event.clientY - rect.top;
    const resolved = currentResolved();
    const hit = widgetAt(x, y, rect.width, rect.height, resolved, scrollX);
    if (hit >= 0) {
      const next = state.clusterWidgets.map((w, i) => (i === hit ? { ...w, signal: sigName } : w));
      commitWidgets(next);
      setNotice(`Bound ${sigName} to cluster gadget.`);
      return;
    }
    if (state.clusterWidgets.length >= MAX_WIDGETS) {
      setNotice(`Cluster holds at most ${MAX_WIDGETS} gadgets.`);
      return;
    }
    const info = signalNamed(sigName);
    const widget: ClusterWidget = info
      ? smartWidgetForSignal(info, newWidgetId())
      : { id: newWidgetId(), kind: "value", signal: sigName };
    const insertAt = targetDrop ?? state.clusterWidgets.length;
    const next = [...state.clusterWidgets];
    next.splice(Math.max(0, Math.min(next.length, insertAt)), 0, widget);
    commitWidgets(next);
    setNotice(`Added ${WIDGET_KIND_LABELS[widget.kind].toLowerCase()} gadget for ${sigName}.`);
  });

  document.addEventListener("mousedown", (event) => {
    if (
      !els.widgetEditor.hidden &&
      !els.widgetEditor.contains(event.target as Node) &&
      event.target !== els.gauges &&
      event.target !== els.btnClusterAdd
    ) {
      closeWidgetEditor();
    }
  });

  els.widgetEditor.addEventListener("keydown", (event) => {
    if (event.key === "Escape") closeWidgetEditor();
  });
}
