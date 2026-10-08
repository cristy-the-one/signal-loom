import { els } from "../dom";
import { formatReading, readoutFor } from "../format";
import { colorFor, signalNamed } from "../model";
import { state } from "../state";

let legendKey: string | null = null;

/** Rebuilds the legend only when a swatch, name, reading or unit it shows differs from the last build. */
export function renderLegend(): void {
  const rows = state.plotted.map((name) => {
    const signal = signalNamed(name);
    const held = state.held.find((item) => item.name === name);
    const readout = readoutFor(signal);
    const reading = held
      ? held.label
        ? `${formatReading(held.value, readout)} ${held.label}`
        : formatReading(held.value, readout)
      : "—".padStart(readout.width);
    return { name, color: colorFor(name), reading, unit: signal?.unit ?? "" };
  });
  els.scaleNote.hidden = rows.length < 2;
  const key = rows.map((row) => [row.color, row.name, row.reading, row.unit].join("\u0001")).join("\u0002");
  if (key === legendKey) return;
  legendKey = key;
  els.legend.replaceChildren(
    ...rows.map((row) => {
      const item = document.createElement("div");
      item.className = "legend-item";
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
      return item;
    }),
  );
}
