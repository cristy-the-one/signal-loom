import { fitCanvas } from "./canvas";
import { readoutFor } from "./format";
import type { BusLoad, SignalInfo, ValueRead } from "./types";

/** The cluster's places for a signal: two dials, a digit, three bars and six lamps. */
export type SlotId =
  | "speed"
  | "rpm"
  | "gear"
  | "bar1"
  | "bar2"
  | "bar3"
  | "lamp1"
  | "lamp2"
  | "lamp3"
  | "lamp4"
  | "lamp5"
  | "lamp6";

/** Slot to the signal the user assigned to it. Saved with the project. */
export type ClusterBindings = Partial<Record<SlotId, string>>;

interface SlotSpec {
  id: SlotId;
  kind: "arc" | "digit" | "bar" | "lamp";
  /** Shown in the assignment chooser. */
  title: string;
  /** Signals the synthetic sample uses here, first match wins. */
  defaults: string[];
  /** Label and fixed scale for a default signal. */
  label: string;
  min: number;
  max: number;
  decimals: number;
  color: string;
  redFrom?: number;
}

export const SLOTS: SlotSpec[] = [
  { id: "speed", kind: "arc", title: "Left dial", defaults: ["VehicleSpeed"], label: "km/h", min: 0, max: 320, decimals: 1, color: "#f2e394" },
  { id: "rpm", kind: "arc", title: "Right dial", defaults: ["EngineRPM", "DisplayedRPM"], label: "rpm", min: 0, max: 9000, decimals: 0, color: "#3ec6ff", redFrom: 7500 },
  { id: "gear", kind: "digit", title: "Digit", defaults: ["Gear", "GearActual"], label: "", min: 0, max: 9, decimals: 0, color: "#f2e394" },
  { id: "bar1", kind: "bar", title: "Bar 1", defaults: ["CoolantTemp"], label: "CLT", min: 40, max: 130, decimals: 0, color: "#ff7a59" },
  { id: "bar2", kind: "bar", title: "Bar 2", defaults: ["OilTemp"], label: "OIL", min: 40, max: 150, decimals: 0, color: "#e6a23c" },
  { id: "bar3", kind: "bar", title: "Bar 3", defaults: ["Soc", "FuelLevel"], label: "SOC", min: 0, max: 100, decimals: 0, color: "#c9a0ff" },
  { id: "lamp1", kind: "lamp", title: "Lamp 1", defaults: ["MilLamp", "TelltaleMil"], label: "MIL", min: 0, max: 1, decimals: 0, color: "#ffb020" },
  { id: "lamp2", kind: "lamp", title: "Lamp 2", defaults: ["AbsActive", "TelltaleAbs"], label: "ABS", min: 0, max: 1, decimals: 0, color: "#ff5a45" },
  { id: "lamp3", kind: "lamp", title: "Lamp 3", defaults: ["EscActive"], label: "ESC", min: 0, max: 1, decimals: 0, color: "#e6a23c" },
  { id: "lamp4", kind: "lamp", title: "Lamp 4", defaults: ["TurnLeft", "TelltaleLeft"], label: "L", min: 0, max: 1, decimals: 0, color: "#7ddea5" },
  { id: "lamp5", kind: "lamp", title: "Lamp 5", defaults: ["TurnRight", "TelltaleRight"], label: "R", min: 0, max: 1, decimals: 0, color: "#7ddea5" },
  { id: "lamp6", kind: "lamp", title: "Lamp 6", defaults: ["DoorFL"], label: "DOOR", min: 0, max: 1, decimals: 0, color: "#7ddea5" },
];

export function slotSpec(id: SlotId): SlotSpec {
  return SLOTS.find((slot) => slot.id === id) ?? SLOTS[0];
}

/** What one slot shows right now. */
export interface SlotReading {
  /** The signal in the slot, or null when nothing fits it. */
  signal: string | null;
  value: number | null;
  label: string;
  min: number;
  max: number;
  decimals: number;
  /** True when the user assigned the signal, not a sample default. */
  assigned: boolean;
}

export type ClusterReadings = Record<SlotId, SlotReading>;

/**
 * Resolve every slot: the user's signal when it is in this log's map,
 * else the first sample default present. An assigned signal scales to its
 * observed range, labelled with its unit or a short name.
 */
export function readingsFrom(
  values: ValueRead[],
  signals: SignalInfo[],
  bindings: ClusterBindings,
): ClusterReadings {
  const byName = new Map(signals.map((signal) => [signal.name, signal]));
  const held = new Map(values.map((value) => [value.name, value.value]));
  const out = {} as ClusterReadings;
  for (const slot of SLOTS) {
    const bound = bindings[slot.id];
    const info = bound ? byName.get(bound) : undefined;
    if (info) {
      const lo = info.min ?? 0;
      const hi = info.max ?? lo + 1;
      out[slot.id] = {
        signal: info.name,
        value: held.get(info.name) ?? null,
        label: slot.kind === "arc" ? info.unit || shortName(info.name, 10) : shortName(info.name, slot.kind === "lamp" ? 6 : 5),
        min: lo,
        max: hi > lo ? hi : lo + 1,
        decimals: Math.min(readoutFor(info).decimals, slot.kind === "bar" ? 1 : 2),
        assigned: true,
      };
      continue;
    }
    const fallback = slot.defaults.find((name) => byName.has(name)) ?? null;
    out[slot.id] = {
      signal: fallback,
      value: fallback ? held.get(fallback) ?? null : null,
      label: slot.id === "bar3" && fallback === "FuelLevel" ? "FUEL" : slot.label,
      min: slot.min,
      max: slot.max,
      decimals: slot.decimals,
      assigned: false,
    };
  }
  return out;
}

/** A label that fits the slot: the name, else its last part (`led_cmd_19` → `CMD19`). */
function shortName(name: string, max: number): string {
  if (name.length <= max) return name.toUpperCase();
  const parts = name.split(/[._\s]+/).filter(Boolean);
  let tail = parts.pop() ?? name;
  // A bare number says little on its own: keep some of the part before it.
  if (/^\d+$/.test(tail) && parts.length) tail = parts.pop()!.slice(0, Math.max(0, max - tail.length)) + tail;
  return (tail.length <= max ? tail : name.slice(0, max)).toUpperCase();
}

/** Where each slot sits on a cluster canvas `h` px tall. */
function slotBoxes(h: number): Record<SlotId, { x: number; y: number; w: number; h: number }> {
  const cy = Math.min(34, h * 0.46);
  const radius = Math.min(22, h * 0.32);
  const arc = (cx: number) => ({ x: cx - radius - 6, y: cy - radius - 6, w: (radius + 6) * 2, h: radius * 2 + 22 });
  const barY = 14;
  const barH = Math.max(20, h - 28);
  const bar = (x: number) => ({ x, y: barY, w: 28, h: barH + 12 });
  const lamp = (index: number) => ({ x: 360 + index * 52, y: h / 2 - 11, w: 46, h: 22 });
  return {
    speed: arc(58),
    rpm: arc(158),
    gear: { x: 108 - 11, y: cy - 12, w: 22, h: 22 },
    bar1: bar(214),
    bar2: bar(254),
    bar3: bar(294),
    lamp1: lamp(0),
    lamp2: lamp(1),
    lamp3: lamp(2),
    lamp4: lamp(3),
    lamp5: lamp(4),
    lamp6: lamp(5),
  };
}

/** The slot under a point in CSS pixels on the cluster canvas, if any. */
export function slotAt(x: number, y: number, w: number, h: number): SlotId | null {
  const boxes = slotBoxes(h);
  for (const slot of SLOTS) {
    const box = boxes[slot.id];
    if (box.x + box.w > w) continue;
    if (x >= box.x && x <= box.x + box.w && y >= box.y && y <= box.y + box.h) return slot.id;
  }
  return null;
}

export function drawGauges(canvas: HTMLCanvasElement, readings: ClusterReadings | null): void {
  const fitted = fitCanvas(canvas);
  if (!fitted) return;
  const { ctx, w, h } = fitted;
  const bg = ctx.createLinearGradient(0, 0, 0, h);
  bg.addColorStop(0, "#161d24");
  bg.addColorStop(1, "#0c1014");
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, w, h);
  ctx.strokeStyle = "rgba(255,255,255,0.04)";
  ctx.beginPath();
  ctx.moveTo(0, 0.5);
  ctx.lineTo(w, 0.5);
  ctx.stroke();

  const empty: SlotReading = { signal: null, value: null, label: "", min: 0, max: 1, decimals: 0, assigned: false };
  const read = (id: SlotId): SlotReading => {
    const reading = readings?.[id];
    return reading ?? { ...empty, label: slotSpec(id).label, min: slotSpec(id).min, max: slotSpec(id).max };
  };
  const cy = Math.min(34, h * 0.46);
  const radius = Math.min(22, h * 0.32);
  for (const [id, cx] of [["speed", 58], ["rpm", 158]] as const) {
    const spec = slotSpec(id);
    const r = read(id);
    const red = r.assigned ? undefined : spec.redFrom;
    arcGauge(ctx, cx, cy, radius, r.value, r.min, r.max, spec.color, r.label, r.decimals, r.value == null, red);
  }
  gearDigit(ctx, 108, cy, read("gear").value, read("gear").assigned);

  const barY = 14;
  const barH = Math.max(20, h - 28);
  for (const [id, x] of [["bar1", 214], ["bar2", 254], ["bar3", 294]] as const) {
    const r = read(id);
    barGauge(ctx, x, barY, 28, barH, r.value, r.min, r.max, slotSpec(id).color, r.label, r.decimals);
  }

  const lamps: SlotId[] = ["lamp1", "lamp2", "lamp3", "lamp4", "lamp5", "lamp6"];
  lamps.forEach((id, index) => {
    const x = 360 + index * 52;
    if (x > w - 46) return;
    const r = read(id);
    lampChip(ctx, x, h / 2 - 11, r.label, r.value != null && r.value > 0.5, slotSpec(id).color);
  });

  // With a real DBC nothing matches the sample's names: say how to fill the cluster.
  const anything = readings != null && Object.values(readings).some((reading) => reading.signal != null);
  const hintX = 360 + lamps.length * 52 + 12;
  if (readings && !anything && hintX < w - 40) {
    ctx.fillStyle = "#8b9aa6";
    ctx.font = "11px 'IBM Plex Sans', sans-serif";
    ctx.textAlign = "left";
    ctx.textBaseline = "middle";
    ctx.fillText("Click a gauge or lamp to assign a signal", hintX, h / 2);
  }
}

function arcGauge(
  ctx: CanvasRenderingContext2D,
  cx: number,
  cy: number,
  radius: number,
  value: number | null,
  min: number,
  max: number,
  color: string,
  unit: string,
  decimals: number,
  empty: boolean,
  redFrom?: number,
): void {
  const start = Math.PI * 0.82;
  const sweep = Math.PI * 1.36;
  ctx.lineCap = "round";
  ctx.lineWidth = 3.5;
  ctx.strokeStyle = "rgba(180, 200, 210, 0.16)";
  ctx.beginPath();
  ctx.arc(cx, cy, radius, start, start + sweep);
  ctx.stroke();
  if (redFrom != null) {
    const ru = (redFrom - min) / (max - min);
    ctx.strokeStyle = "rgba(255, 77, 46, 0.7)";
    ctx.beginPath();
    ctx.arc(cx, cy, radius, start + sweep * ru, start + sweep);
    ctx.stroke();
  }
  const shown = value ?? min;
  const u = Math.min(1, Math.max(0, (shown - min) / (max - min)));
  ctx.strokeStyle = empty ? "rgba(180, 200, 210, 0.25)" : color;
  ctx.beginPath();
  ctx.arc(cx, cy, radius, start, start + sweep * u);
  ctx.stroke();
  const ang = start + sweep * u;
  ctx.strokeStyle = empty ? "rgba(231,238,243,0.35)" : "#e7eef3";
  ctx.lineWidth = 1.2;
  ctx.beginPath();
  ctx.moveTo(cx + Math.cos(ang) * 6, cy + Math.sin(ang) * 6);
  ctx.lineTo(cx + Math.cos(ang) * (radius - 1), cy + Math.sin(ang) * (radius - 1));
  ctx.stroke();
  ctx.fillStyle = empty ? "#6d7c88" : "#e7eef3";
  ctx.font = "500 13px 'IBM Plex Mono', ui-monospace, monospace";
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(empty || value == null ? "—" : value.toFixed(decimals), cx, cy + 8);
  ctx.fillStyle = "#8b9aa6";
  ctx.font = "9px 'IBM Plex Mono', ui-monospace, monospace";
  ctx.fillText(unit, cx, cy + radius + 8);
}

function gearDigit(ctx: CanvasRenderingContext2D, x: number, y: number, gear: number | null, assigned = false): void {
  ctx.fillStyle = "#0b0f13";
  ctx.strokeStyle = "#2a343d";
  ctx.lineWidth = 1;
  ctx.beginPath();
  roundRect(ctx, x - 11, y - 12, 22, 22, 2);
  ctx.fill();
  ctx.stroke();
  ctx.fillStyle = "#f2e394";
  ctx.font = "600 13px 'IBM Plex Mono', ui-monospace, monospace";
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  const label =
    gear == null ? "–" : assigned ? String(Math.round(gear)).slice(0, 2) : gear < 0.5 ? "N" : String(Math.round(gear));
  ctx.fillText(label, x, y);
}

function barGauge(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  width: number,
  height: number,
  value: number | null,
  min: number,
  max: number,
  color: string,
  label: string,
  decimals = 0,
): void {
  ctx.fillStyle = "#0b0f13";
  ctx.strokeStyle = "#243039";
  ctx.lineWidth = 1;
  ctx.beginPath();
  roundRect(ctx, x, y, width, height, 2);
  ctx.fill();
  ctx.stroke();
  if (value != null) {
    const u = Math.min(1, Math.max(0, (value - min) / (max - min)));
    const fillH = Math.max(2, (height - 4) * u);
    ctx.fillStyle = color;
    ctx.fillRect(x + 3, y + height - 2 - fillH, width - 6, fillH);
  }
  ctx.fillStyle = "#e7eef3";
  ctx.font = "10px 'IBM Plex Mono', ui-monospace, monospace";
  ctx.textAlign = "center";
  ctx.textBaseline = "top";
  ctx.fillText(value == null ? "—" : value.toFixed(decimals), x + width / 2, y + 3);
  ctx.fillStyle = "#8b9aa6";
  ctx.font = "9px 'IBM Plex Mono', ui-monospace, monospace";
  ctx.fillText(label, x + width / 2, y + height + 2);
}

function lampChip(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  label: string,
  lit: boolean,
  color: string,
): void {
  ctx.beginPath();
  roundRect(ctx, x, y, 46, 22, 2);
  ctx.fillStyle = lit ? color : "#12181e";
  ctx.fill();
  ctx.strokeStyle = lit ? color : "#2a343d";
  ctx.stroke();
  ctx.fillStyle = lit ? "#121418" : "#6d7c88";
  ctx.font = "600 10px 'IBM Plex Sans', sans-serif";
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(label, x + 23, y + 11);
}

function roundRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
): void {
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

export function drawBus(canvas: HTMLCanvasElement, load: BusLoad | null): void {
  const fitted = fitCanvas(canvas);
  if (!fitted) return;
  const { ctx, w, h } = fitted;
  ctx.fillStyle = "#0b0e12";
  ctx.fillRect(0, 0, w, h);
  const pad = 8;
  ctx.font = "10px 'IBM Plex Mono', ui-monospace, monospace";
  ctx.textBaseline = "middle";
  ctx.textAlign = "left";
  ctx.fillStyle = "#d5dee6";
  // Each field is padded to a fixed width, and the bar starts after the
  // widest possible label, so neither moves as the numbers change.
  const rate = load ? load.rate.toFixed(0).padStart(4) : "   —";
  const pct = load ? (load.load * 100).toFixed(1).padStart(5) : "    —";
  const frames = load ? String(load.frames).padStart(4) : "   —";
  ctx.fillText(`${rate} f/s   ${pct}% of 500 kbit/s   ${frames} fr / 1 s`, pad, h / 2);
  const textW = ctx.measureText("9999 f/s   100.0% of 500 kbit/s   9999 fr / 1 s").width;
  const barX = pad + textW + 12;
  const barW = Math.max(24, w - barX - pad);
  const barY = Math.max(3, (h - 6) / 2);
  ctx.fillStyle = "#1a2229";
  ctx.fillRect(barX, barY, barW, 6);
  const fraction = load ? Math.min(1, Math.max(0, load.load)) : 0;
  ctx.fillStyle = fraction > 0.7 ? "#ff5a45" : fraction > 0.4 ? "#e6a23c" : "#3ec6ff";
  ctx.fillRect(barX, barY, barW * fraction, 6);
}
