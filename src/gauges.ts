import { fitCanvas, roundRect } from "./canvas";
import { readoutFor } from "./format";
import {
  AMBER,
  CORAL,
  CYAN,
  EDGE,
  GREEN,
  MONO,
  ORANGE,
  RED,
  RULE,
  SANS,
  SURFACE,
  TEXT,
  TEXT_FAINT,
  TEXT_MUTED,
  VIOLET,
  WELL,
  YELLOW,
} from "./theme";
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
  { id: "speed", kind: "arc", title: "Left dial", defaults: ["VehicleSpeed"], label: "km/h", min: 0, max: 320, decimals: 1, color: YELLOW },
  { id: "rpm", kind: "arc", title: "Right dial", defaults: ["EngineRPM", "DisplayedRPM"], label: "rpm", min: 0, max: 9000, decimals: 0, color: CYAN, redFrom: 7500 },
  { id: "gear", kind: "digit", title: "Digit", defaults: ["Gear", "GearActual"], label: "", min: 0, max: 9, decimals: 0, color: YELLOW },
  { id: "bar1", kind: "bar", title: "Bar 1", defaults: ["CoolantTemp"], label: "CLT", min: 40, max: 130, decimals: 0, color: CORAL },
  { id: "bar2", kind: "bar", title: "Bar 2", defaults: ["OilTemp"], label: "OIL", min: 40, max: 150, decimals: 0, color: AMBER },
  { id: "bar3", kind: "bar", title: "Bar 3", defaults: ["Soc", "FuelLevel"], label: "SOC", min: 0, max: 100, decimals: 0, color: VIOLET },
  { id: "lamp1", kind: "lamp", title: "Lamp 1", defaults: ["MilLamp", "TelltaleMil"], label: "MIL", min: 0, max: 1, decimals: 0, color: ORANGE },
  { id: "lamp2", kind: "lamp", title: "Lamp 2", defaults: ["AbsActive", "TelltaleAbs"], label: "ABS", min: 0, max: 1, decimals: 0, color: RED },
  { id: "lamp3", kind: "lamp", title: "Lamp 3", defaults: ["EscActive"], label: "ESC", min: 0, max: 1, decimals: 0, color: AMBER },
  { id: "lamp4", kind: "lamp", title: "Lamp 4", defaults: ["TurnLeft", "TelltaleLeft"], label: "L", min: 0, max: 1, decimals: 0, color: GREEN },
  { id: "lamp5", kind: "lamp", title: "Lamp 5", defaults: ["TurnRight", "TelltaleRight"], label: "R", min: 0, max: 1, decimals: 0, color: GREEN },
  { id: "lamp6", kind: "lamp", title: "Lamp 6", defaults: ["DoorFL"], label: "DOOR", min: 0, max: 1, decimals: 0, color: GREEN },
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

/** Horizontal anchor per slot: a dial's or the digit's centre, a bar's or lamp's left edge. */
const ANCHOR = { speed: 58, rpm: 158, gear: 108, bar1: 214, bar2: 254, bar3: 294 } as const;
const LAMP_X = 360;
const LAMP_PITCH = 52;
const LAMP_W = 46;
const LAMP_H = 22;
const BAR_W = 28;
const DIGIT = 22;

interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Where every slot sits on a cluster canvas `h` px tall. Drawing and hit-testing both read it. */
interface Geometry {
  /** Centre line of the dials and the digit. */
  cy: number;
  radius: number;
  barY: number;
  barH: number;
  boxes: Record<SlotId, Box>;
}

function geometry(h: number): Geometry {
  const cy = Math.min(34, h * 0.46);
  const radius = Math.min(22, h * 0.32);
  const barY = 14;
  const barH = Math.max(20, h - 28);
  const dial = (cx: number): Box => ({ x: cx - radius - 6, y: cy - radius - 6, w: (radius + 6) * 2, h: radius * 2 + 22 });
  const bar = (x: number): Box => ({ x, y: barY, w: BAR_W, h: barH + 12 });
  const lamp = (index: number): Box => ({ x: LAMP_X + index * LAMP_PITCH, y: h / 2 - 11, w: LAMP_W, h: LAMP_H });
  return {
    cy,
    radius,
    barY,
    barH,
    boxes: {
      speed: dial(ANCHOR.speed),
      rpm: dial(ANCHOR.rpm),
      gear: { x: ANCHOR.gear - DIGIT / 2, y: cy - 12, w: DIGIT, h: DIGIT },
      bar1: bar(ANCHOR.bar1),
      bar2: bar(ANCHOR.bar2),
      bar3: bar(ANCHOR.bar3),
      lamp1: lamp(0),
      lamp2: lamp(1),
      lamp3: lamp(2),
      lamp4: lamp(3),
      lamp5: lamp(4),
      lamp6: lamp(5),
    },
  };
}

/** The slot under a point in CSS pixels on the cluster canvas, if any. */
export function slotAt(x: number, y: number, w: number, h: number): SlotId | null {
  const { boxes } = geometry(h);
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
  const g = geometry(h);
  for (const id of ["speed", "rpm"] as const) {
    const spec = slotSpec(id);
    const r = read(id);
    const red = r.assigned ? undefined : spec.redFrom;
    arcGauge(ctx, ANCHOR[id], g.cy, g.radius, r.value, r.min, r.max, spec.color, r.label, r.decimals, r.value == null, red);
  }
  const gear = read("gear");
  gearDigit(ctx, g.boxes.gear, ANCHOR.gear, g.cy, gear.value, gear.assigned);

  for (const id of ["bar1", "bar2", "bar3"] as const) {
    const r = read(id);
    barGauge(ctx, g.boxes[id].x, g.barY, BAR_W, g.barH, r.value, r.min, r.max, slotSpec(id).color, r.label, r.decimals);
  }

  const lamps: SlotId[] = ["lamp1", "lamp2", "lamp3", "lamp4", "lamp5", "lamp6"];
  for (const id of lamps) {
    const box = g.boxes[id];
    if (box.x + box.w > w) continue;
    const r = read(id);
    lampChip(ctx, box, r.label, r.value != null && r.value > 0.5, slotSpec(id).color);
  }

  // With a real DBC nothing matches the sample's names: say how to fill the cluster.
  const anything = readings != null && Object.values(readings).some((reading) => reading.signal != null);
  const hintX = LAMP_X + lamps.length * LAMP_PITCH + 12;
  if (readings && !anything && hintX < w - 40) {
    ctx.fillStyle = TEXT_MUTED;
    ctx.font = `11px ${SANS}`;
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
  ctx.strokeStyle = empty ? "rgba(231,238,243,0.35)" : TEXT;
  ctx.lineWidth = 1.2;
  ctx.beginPath();
  ctx.moveTo(cx + Math.cos(ang) * 6, cy + Math.sin(ang) * 6);
  ctx.lineTo(cx + Math.cos(ang) * (radius - 1), cy + Math.sin(ang) * (radius - 1));
  ctx.stroke();
  ctx.fillStyle = empty ? TEXT_FAINT : TEXT;
  ctx.font = `500 13px ${MONO}`;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(empty || value == null ? "—" : value.toFixed(decimals), cx, cy + 8);
  ctx.fillStyle = TEXT_MUTED;
  ctx.font = `9px ${MONO}`;
  ctx.fillText(unit, cx, cy + radius + 8);
}

function gearDigit(
  ctx: CanvasRenderingContext2D,
  box: Box,
  x: number,
  y: number,
  gear: number | null,
  assigned = false,
): void {
  ctx.fillStyle = WELL;
  ctx.strokeStyle = EDGE;
  ctx.lineWidth = 1;
  ctx.beginPath();
  roundRect(ctx, box.x, box.y, box.w, box.h, 2);
  ctx.fill();
  ctx.stroke();
  ctx.fillStyle = YELLOW;
  ctx.font = `600 13px ${MONO}`;
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
  ctx.fillStyle = WELL;
  ctx.strokeStyle = RULE;
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
  ctx.fillStyle = TEXT;
  ctx.font = `10px ${MONO}`;
  ctx.textAlign = "center";
  ctx.textBaseline = "top";
  ctx.fillText(value == null ? "—" : value.toFixed(decimals), x + width / 2, y + 3);
  ctx.fillStyle = TEXT_MUTED;
  ctx.font = `9px ${MONO}`;
  ctx.fillText(label, x + width / 2, y + height + 2);
}

function lampChip(ctx: CanvasRenderingContext2D, box: Box, label: string, lit: boolean, color: string): void {
  ctx.beginPath();
  roundRect(ctx, box.x, box.y, box.w, box.h, 2);
  ctx.fillStyle = lit ? color : SURFACE;
  ctx.fill();
  ctx.strokeStyle = lit ? color : EDGE;
  ctx.stroke();
  ctx.fillStyle = lit ? "#121418" : TEXT_FAINT;
  ctx.font = `600 10px ${SANS}`;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(label, box.x + box.w / 2, box.y + box.h / 2);
}

export function drawBus(canvas: HTMLCanvasElement, load: BusLoad | null): void {
  const fitted = fitCanvas(canvas);
  if (!fitted) return;
  const { ctx, w, h } = fitted;
  ctx.fillStyle = "#0b0e12";
  ctx.fillRect(0, 0, w, h);
  const pad = 8;
  ctx.font = `10px ${MONO}`;
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
  ctx.fillStyle = fraction > 0.7 ? RED : fraction > 0.4 ? AMBER : CYAN;
  ctx.fillRect(barX, barY, barW * fraction, 6);
}
