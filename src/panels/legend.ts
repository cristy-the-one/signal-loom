import { els } from "../dom";
import { formatReading, readoutFor } from "../format";
import { buildLaneSpecs, cycleSignalLane } from "../lanes";
import { colorFor, signalNamed } from "../model";
import { refresh, refreshOverview, setPlotted } from "../query";
import { renderDirty } from "../readout";
import { draw, renderChrome, state } from "../state";

let legendKey: string | null = null;

/** Rebuilds the legend only when a swatch, name, reading, unit, solo state, or lane badge differs. */
export function renderLegend(): void {
  if (state.plotted.length === 0) {
    els.legend.hidden = true;
    if (legendKey !== "") {
      legendKey = "";
      els.legend.replaceChildren();
    }
    return;
  }
  els.legend.hidden = false;

  const heldByName = new Map(state.held.map((item) => [item.name, item]));
  const badgeByName = new Map<string, string>();
  if (state.laneMode === "grouped") {
    for (const spec of buildLaneSpecs(state.plotted, "grouped", state.signalLanes, signalNamed)) {
      for (const sig of spec.signals) badgeByName.set(sig, spec.badge);
    }
  }

  const rows = state.plotted.map((name) => {
    const signal = signalNamed(name);
    const held = heldByName.get(name);
    const readout = readoutFor(signal);
    const reading = held
      ? held.label
        ? `${formatReading(held.value, readout)} ${held.label}`
        : formatReading(held.value, readout)
      : "—".padStart(readout.width);
    const badge = badgeByName.get(name) ?? "";
    const pinned = (state.signalLanes[name] ?? 0) > 0;
    const isSolo = state.solo === name;
    const isDim = state.solo != null && state.solo !== name;
    return {
      name,
      color: colorFor(name),
      reading,
      unit: signal?.unit ?? "",
      badge,
      pinned,
      isSolo,
      isDim,
    };
  });

  const key = `${state.laneMode}\u0003` + rows
    .map((row) =>
      [row.color, row.name, row.reading, row.unit, row.badge, row.pinned ? 1 : 0, row.isSolo ? 1 : 0, row.isDim ? 1 : 0].join(
        "\u0001",
      ),
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
      item.title = row.isSolo ? `Click to show all plotted signals` : `Click to solo ${row.name}`;
      item.addEventListener("click", () => {
        state.solo = state.solo === row.name ? null : row.name;
        draw();
      });

      const swatch = document.createElement("i");
      swatch.style.background = row.color;
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
