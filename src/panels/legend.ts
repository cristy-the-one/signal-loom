import { els } from "../dom";
import { formatReading, readoutFor } from "../format";
import {
  TRACE_STYLES,
  TRACE_STYLE_META,
  buildLaneSpecs,
  cycleSignalLane,
  resolveTraceStyle,
} from "../lanes";
import { colorFor, signalNamed } from "../model";
import { refresh, refreshOverview, setPlotted } from "../query";
import { renderDirty } from "../readout";
import { draw, renderChrome, state } from "../state";
import type { TraceStyle } from "../types";
import { closeColorPicker, openColorPicker } from "./color-picker";

let legendKey: string | null = null;
let styleMenuSignal: string | null = null;
let styleMenuBound = false;

function closeStyleMenu(): void {
  styleMenuSignal = null;
  els.styleMenu.hidden = true;
}

function ensureStyleMenuBound(): void {
  if (styleMenuBound) return;
  styleMenuBound = true;
  document.addEventListener("mousedown", (event) => {
    if (!els.styleMenu.hidden && !els.styleMenu.contains(event.target as Node)) {
      closeStyleMenu();
    }
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !els.styleMenu.hidden) closeStyleMenu();
  });
}

function openStyleMenu(signalName: string, anchor: HTMLElement): void {
  ensureStyleMenuBound();
  if (styleMenuSignal === signalName && !els.styleMenu.hidden) {
    closeStyleMenu();
    return;
  }
  styleMenuSignal = signalName;
  const info = signalNamed(signalName);
  const currentOverride: TraceStyle = state.signalStyles[signalName] ?? "auto";
  const autoResolved = resolveTraceStyle(info);

  els.styleMenu.replaceChildren(
    ...TRACE_STYLES.map((style) => {
      const meta = TRACE_STYLE_META[style];
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = currentOverride === style ? "style-opt is-on" : "style-opt";
      const head = document.createElement("span");
      head.className = "style-opt-name";
      head.textContent = style === "auto" ? `Auto (${TRACE_STYLE_META[autoResolved].short})` : meta.label;
      const desc = document.createElement("span");
      desc.className = "style-opt-desc";
      desc.textContent = meta.desc;
      btn.append(head, desc);
      btn.addEventListener("click", (event) => {
        event.stopPropagation();
        const next = { ...state.signalStyles };
        if (style === "auto") delete next[signalName];
        else next[signalName] = style;
        state.signalStyles = next;
        state.dirty = true;
        closeStyleMenu();
        renderDirty();
        draw();
      });
      return btn;
    }),
  );

  const rect = anchor.getBoundingClientRect();
  const menuW = 216;
  const left = Math.max(8, Math.min(rect.left, window.innerWidth - menuW - 8));
  const top = Math.min(rect.bottom + 4, window.innerHeight - 250);
  els.styleMenu.style.left = `${left}px`;
  els.styleMenu.style.top = `${top}px`;
  els.styleMenu.hidden = false;
}

/** Rebuilds the legend only when a swatch, name, reading, unit, style, solo state, or lane badge differs. */
export function renderLegend(mathValues?: ReadonlyMap<string, number>): void {
  if (state.plotted.length === 0) {
    els.legend.hidden = true;
    closeStyleMenu();
    closeColorPicker();
    if (legendKey !== "") {
      legendKey = "";
      els.legend.replaceChildren();
    }
    return;
  }
  els.legend.hidden = false;

  const heldByName = new Map(state.held.map((item) => [item.name, item]));
  const specs = buildLaneSpecs(state.plotted, state.laneMode, state.signalLanes, signalNamed, state.signalStyles);
  const badgeByName = new Map<string, string>();
  if (state.laneMode === "grouped") {
    for (const spec of specs) {
      for (const sig of spec.signals) badgeByName.set(sig, spec.badge);
    }
  }
  const focusedLaneSignals = state.focusedLane
    ? new Set(specs.find((s) => s.id === state.focusedLane)?.signals ?? [])
    : null;

  const rows = state.plotted.map((name) => {
    const signal = signalNamed(name);
    const held = heldByName.get(name);
    const mathVal = held ? undefined : mathValues?.get(name);
    const readout = readoutFor(signal);
    const reading = held
      ? held.label
        ? `${formatReading(held.value, readout)} ${held.label}`
        : formatReading(held.value, readout)
      : mathVal != null
        ? formatReading(mathVal, readout)
        : "—".padStart(readout.width);
    const badge = badgeByName.get(name) ?? "";
    const pinned = (state.signalLanes[name] ?? 0) > 0;
    const styleOverride = state.signalStyles[name];
    const resolvedStyle = resolveTraceStyle(signal, styleOverride);
    const stylePinned = styleOverride != null && styleOverride !== "auto";
    const isSolo = state.solo === name || (focusedLaneSignals != null && focusedLaneSignals.has(name));
    const isDim =
      (state.solo != null && state.solo !== name) ||
      (focusedLaneSignals != null && focusedLaneSignals.size > 0 && !focusedLaneSignals.has(name));
    return {
      name,
      color: colorFor(name),
      reading,
      unit: signal?.unit ?? "",
      badge,
      pinned,
      resolvedStyle,
      stylePinned,
      isSolo,
      isDim,
    };
  });

  const key =
    `${state.laneMode}\u0003` +
    rows
      .map((row) =>
        [
          row.color,
          row.name,
          row.reading,
          row.unit,
          row.badge,
          row.pinned ? 1 : 0,
          row.resolvedStyle,
          row.stylePinned ? 1 : 0,
          row.isSolo ? 1 : 0,
          row.isDim ? 1 : 0,
        ].join("\u0001"),
      )
      .join("\u0002");
  if (key === legendKey) return;
  legendKey = key;

  els.legend.replaceChildren(
    ...rows.map((row) => {
      const item = document.createElement("div");
      item.className = "legend-item";
      if (row.isSolo) item.classList.add("is-solo");
      if (row.isDim) item.classList.add("is-dim");
      item.title = row.isSolo
        ? `Click to clear focus (Esc)`
        : `Click to focus ${row.name} (dims other signals without hiding them)`;
      item.addEventListener("click", () => {
        state.focusedLane = null;
        state.solo = state.solo === row.name ? null : row.name;
        draw();
      });

      const swatch = document.createElement("button");
      swatch.type = "button";
      swatch.className = "legend-swatch";
      swatch.style.background = row.color;
      swatch.title = `Change color for ${row.name}`;
      swatch.setAttribute("aria-label", `Change color for ${row.name}`);
      swatch.addEventListener("click", (event) => {
        event.stopPropagation();
        closeStyleMenu();
        openColorPicker(row.name, swatch);
      });
      const label = document.createElement("span");
      label.className = "name";
      label.textContent = row.name;
      const reading = document.createElement("span");
      reading.className = "val";
      reading.textContent = row.reading;
      const unit = document.createElement("span");
      unit.className = "unit";
      unit.textContent = row.unit;
      item.append(swatch, label, reading, unit);

      const styleBtn = document.createElement("button");
      styleBtn.type = "button";
      styleBtn.className = row.stylePinned ? "legend-style is-pinned" : "legend-style";
      styleBtn.textContent = TRACE_STYLE_META[row.resolvedStyle].short;
      styleBtn.title = `Visualization: ${TRACE_STYLE_META[row.resolvedStyle].label} — click to change (Step, Line, Area, State bar, Logic flag, Heat strip)`;
      styleBtn.addEventListener("click", (event) => {
        event.stopPropagation();
        openStyleMenu(row.name, styleBtn);
      });
      item.append(styleBtn);

      if (row.badge) {
        const laneBtn = document.createElement("button");
        laneBtn.type = "button";
        laneBtn.className = row.pinned ? "legend-lane is-pinned" : "legend-lane";
        laneBtn.textContent = row.badge;
        laneBtn.title = row.pinned
          ? `Pinned to group ${state.signalLanes[row.name]} — click to move to next track or reset to auto`
          : `Auto-grouped on ${row.badge} — click to assign a custom track group`;
        laneBtn.addEventListener("click", (event) => {
          event.stopPropagation();
          state.signalLanes = cycleSignalLane(row.name, state.plotted, state.signalLanes);
          state.dirty = true;
          renderDirty();
          draw();
        });
        item.append(laneBtn);
      }

      const close = document.createElement("button");
      close.type = "button";
      close.className = "legend-x";
      close.textContent = "×";
      close.ariaLabel = `Hide ${row.name}`;
      close.title = `Hide ${row.name}`;
      close.addEventListener("click", (event) => {
        event.stopPropagation();
        setPlotted(state.plotted.filter((n) => n !== row.name));
        state.dirty = true;
        renderDirty();
        renderChrome();
        draw();
        void refresh();
        void refreshOverview();
      });
      item.append(close);

      return item;
    }),
  );
}
