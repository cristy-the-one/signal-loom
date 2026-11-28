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
  PINK,
  RED,
  RULE,
  SANS,
  SKY,
  SURFACE,
  TEAL,
  TEXT,
  TEXT_FAINT,
  TEXT_MUTED,
  VIOLET,
  WELL,
  YELLOW,
} from "./theme";
import type { BusLoad, ClusterWidget, LampOp, SignalInfo, ValueRead, WidgetKind } from "./types";

/** The legacy cluster's places for a signal: two dials, a digit, three bars and six lamps. */
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

/** Slot to the signal the user assigned to it. Saved with legacy projects. */
export type ClusterBindings = Partial<Record<SlotId, string>>;

export const WIDGET_KINDS: readonly WidgetKind[] = ["arc", "bar", "digit", "value", "lamp"];

export const WIDGET_KIND_LABELS: Record<WidgetKind, string> = {
  arc: "Dial",
  bar: "Bar",
  digit: "Digit",
  value: "Readout",
  lamp: "Lamp",
};

export const WIDGET_COLORS: readonly { name: string; hex: string }[] = [
  { name: "Cyan", hex: CYAN },
  { name: "Yellow", hex: YELLOW },
  { name: "Amber", hex: AMBER },
  { name: "Coral", hex: CORAL },
  { name: "Red", hex: RED },
  { name: "Green", hex: GREEN },
  { name: "Teal", hex: TEAL },
  { name: "Violet", hex: VIOLET },
  { name: "Pink", hex: PINK },
  { name: "Sky", hex: SKY },
];

export const LAMP_OPS: readonly LampOp[] = [">", ">=", "<", "<=", "==", "!="];

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
  { id: "gear", kind: "digit", title: "Digit", defaults: ["Gear", "GearActual"], label: "", min: 0, max: 9, decimals: 0, color: YELLOW },
  { id: "rpm", kind: "arc", title: "Right dial", defaults: ["EngineRPM", "DisplayedRPM"], label: "rpm", min: 0, max: 9000, decimals: 0, color: CYAN, redFrom: 7500 },
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

const SLOT_BY_ID = new Map<string, SlotSpec>(SLOTS.map((slot) => [slot.id, slot]));

export function slotSpec(id: SlotId): SlotSpec {
  return SLOT_BY_ID.get(id) ?? SLOTS[0];
}

/** Build the default 12-widget preset, optionally applying legacy slot bindings. */
export function defaultClusterWidgets(legacy?: ClusterBindings): ClusterWidget[] {
  return SLOTS.map((slot) => ({
    id: slot.id,
    kind: slot.kind,
    signal: legacy?.[slot.id] ?? null,
    color: slot.color,
  }));
}

/** Create a clean, smart-typed widget for any dropped or newly added signal. */
export function smartWidgetForSignal(signal: SignalInfo, id: string): ClusterWidget {
  const unit = signal.unit.trim();
  const lo = signal.min ?? 0;
  const hi = signal.max ?? lo + 1;
  const range = hi - lo;
  if (!unit && signal.labels && signal.labels.length > 0) {
    return { id, kind: "digit", signal: signal.name, label: shortName(signal.name, 7), color: YELLOW };
  }
  if (!unit && signal.step === 1 && lo >= 0 && hi <= 1) {
    return {
      id,
      kind: "lamp",
      signal: signal.name,
      label: shortName(signal.name, 6),
      color: GREEN,
      lampOp: ">",
      lampThreshold: 0.5,
    };
  }
  if (!unit && signal.step === 1 && range <= 12 && lo >= 0 && Number.isInteger(lo) && Number.isInteger(hi)) {
    return { id, kind: "digit", signal: signal.name, label: shortName(signal.name, 6), color: YELLOW };
  }
  if (unit === "%" || unit.includes("°") || /temp|fuel|soc|level/i.test(signal.name)) {
    return { id, kind: "bar", signal: signal.name, color: unit === "%" ? VIOLET : CORAL };
  }
  if (/rpm|speed|vel|boost|press|torque/i.test(signal.name)) {
    return { id, kind: "arc", signal: signal.name, color: CYAN };
  }
  return { id, kind: "value", signal: signal.name, color: SKY };
}

/** Strip undefined fields for clean `.loom` persistence. */
export function serializeClusterWidgets(widgets: readonly ClusterWidget[]): ClusterWidget[] {
  return widgets.map((w) => {
    const out: ClusterWidget = { id: w.id, kind: w.kind };
    if (w.signal) out.signal = w.signal;
    if (w.label?.trim()) out.label = w.label.trim();
    if (w.color) out.color = w.color;
    if (w.min != null && Number.isFinite(w.min)) out.min = w.min;
    if (w.max != null && Number.isFinite(w.max)) out.max = w.max;
    if (w.decimals != null && Number.isInteger(w.decimals)) out.decimals = w.decimals;
    if (w.warnFrom != null && Number.isFinite(w.warnFrom)) out.warnFrom = w.warnFrom;
    if (w.lampOp) out.lampOp = w.lampOp;
    if (w.lampThreshold != null && Number.isFinite(w.lampThreshold)) out.lampThreshold = w.lampThreshold;
    return out;
  });
}

/** What one resolved widget shows right now. */
export interface ResolvedWidget {
  widget: ClusterWidget;
  signal: string | null;
  value: number | null;
  enumLabel: string | null;
  unit: string;
  label: string;
  autoLabel: string;
  min: number;
  max: number;
  decimals: number;
  color: string;
  warnFrom?: number;
  lit: boolean;
  assigned: boolean;
}

export function evalLamp(value: number | null, op: LampOp, threshold: number): boolean {
  if (value == null || !Number.isFinite(value)) return false;
  switch (op) {
    case ">": return value > threshold;
    case ">=": return value >= threshold;
    case "<": return value < threshold;
    case "<=": return value <= threshold;
    case "==": return Math.abs(value - threshold) <= 1e-6;
    case "!=": return Math.abs(value - threshold) > 1e-6;
  }
}

function lookupEnumLabel(info: SignalInfo | undefined, value: number | null, heldLabel?: string): string | null {
  if (heldLabel) return heldLabel;
  if (value == null || !info?.labels?.length) return null;
  const rounded = Math.round(value);
  if (Math.abs(value - rounded) > 1e-6) return null;
  return info.labels.find((item) => item.value === rounded)?.label ?? null;
}

/**
 * Resolve every widget in the cluster against live held values, signals, and
 * optional evaluated math channel values.
 */
export function resolveWidgets(
  widgets: readonly ClusterWidget[],
  values: ValueRead[],
  signals: SignalInfo[],
  mathValues?: ReadonlyMap<string, number>,
): ResolvedWidget[] {
  const byName = new Map(signals.map((signal) => [signal.name, signal]));
  const held = new Map(values.map((value) => [value.name, value]));

  return widgets.map((widget) => {
    const preset = SLOT_BY_ID.get(widget.id);
    const bound = widget.signal?.trim() || null;
    const boundInfo = bound ? byName.get(bound) : undefined;
    const color = widget.color || preset?.color || CYAN;
    const lampOp: LampOp = widget.lampOp ?? ">";
    const lampThreshold = widget.lampThreshold ?? 0.5;

    if (bound && (boundInfo || mathValues?.has(bound))) {
      const h = held.get(bound);
      const val = h ? h.value : (mathValues?.get(bound) ?? null);
      const unit = boundInfo?.unit ?? "";
      const autoLo = boundInfo?.min ?? 0;
      const autoHi = boundInfo?.max ?? autoLo + 1;
      const min = widget.min ?? autoLo;
      const rawMax = widget.max ?? autoHi;
      const max = rawMax > min ? rawMax : min + 1;
      const autoDec = Math.min(readoutFor(boundInfo).decimals, widget.kind === "bar" ? 1 : 2);
      const decimals = widget.decimals ?? autoDec;
      const autoLabel =
        widget.kind === "arc"
          ? unit || shortName(bound, 10)
          : widget.kind === "value"
            ? shortName(bound, 12)
            : shortName(bound, widget.kind === "lamp" ? 7 : 5);
      const label = widget.label?.trim() || autoLabel;
      const enumLabel = lookupEnumLabel(boundInfo, val, h?.label);
      return {
        widget,
        signal: bound,
        value: val,
        enumLabel,
        unit,
        label,
        autoLabel,
        min,
        max,
        decimals,
        color,
        warnFrom: widget.warnFrom,
        lit: evalLamp(val, lampOp, lampThreshold),
        assigned: true,
      };
    }

    // Fallback to sample preset defaults when unbound or when the bound signal is absent from this log.
    const fallback = preset ? preset.defaults.find((name) => byName.has(name)) ?? null : null;
    const fbInfo = fallback ? byName.get(fallback) : undefined;
    const fbHeld = fallback ? held.get(fallback) : undefined;
    const val = fbHeld?.value ?? null;
    const autoLabel = preset
      ? preset.id === "bar3" && fallback === "FuelLevel"
        ? "FUEL"
        : preset.label
      : widget.kind === "lamp"
        ? "LAMP"
        : "—";
    const label = widget.label?.trim() || autoLabel;
    const min = widget.min ?? preset?.min ?? 0;
    const rawMax = widget.max ?? preset?.max ?? 100;
    const max = rawMax > min ? rawMax : min + 1;
    const decimals = widget.decimals ?? preset?.decimals ?? 0;
    const warnFrom = widget.warnFrom ?? preset?.redFrom;
    const enumLabel = lookupEnumLabel(fbInfo, val, fbHeld?.label);

    return {
      widget,
      signal: fallback,
      value: val,
      enumLabel,
      unit: fbInfo?.unit ?? "",
      label,
      autoLabel,
      min,
      max,
      decimals,
      color,
      warnFrom,
      lit: evalLamp(val, lampOp, lampThreshold),
      assigned: false,
    };
  });
}

/** What one legacy slot shows right now (kept for backward compatibility & tests). */
export interface SlotReading {
  signal: string | null;
  value: number | null;
  label: string;
  min: number;
  max: number;
  decimals: number;
  assigned: boolean;
}

export type ClusterReadings = Record<SlotId, SlotReading>;

/** Resolve legacy slots; delegates to the unified widget resolver. */
export function readingsFrom(
  values: ValueRead[],
  signals: SignalInfo[],
  bindings: ClusterBindings,
): ClusterReadings {
  const resolved = resolveWidgets(defaultClusterWidgets(bindings), values, signals);
  const out = {} as ClusterReadings;
  for (const item of resolved) {
    out[item.widget.id as SlotId] = {
      signal: item.signal,
      value: item.value,
      label: item.label,
      min: item.min,
      max: item.max,
      decimals: item.decimals,
      assigned: item.assigned,
    };
  }
  return out;
}

/** A label that fits the slot: the name, else its last part (`led_cmd_19` → `CMD19`). */
export function shortName(name: string, max: number): string {
  if (name.length <= max) return name.toUpperCase();
  const parts = name.split(/[._\s]+/).filter(Boolean);
  let tail = parts.pop() ?? name;
  // A bare number says little on its own: keep some of the part before it.
  if (/^\d+$/.test(tail) && parts.length) tail = parts.pop()!.slice(0, Math.max(0, max - tail.length)) + tail;
  return (tail.length <= max ? tail : name.slice(0, max)).toUpperCase();
}

export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface WidgetLayout {
  index: number;
  box: Box;
  cx: number;
  cy: number;
  radius: number;
  barY: number;
  barH: number;
}

const STRIP_LEFT = 14;
const WIDGET_GAP = 10;
const BAR_W = 30;
const DIGIT_SIZE = 24;
const LAMP_H = 24;

function widgetWidth(item: ResolvedWidget): number {
  switch (item.widget.kind) {
    case "arc":
      return 64;
    case "digit":
      return Math.max(30, Math.min(64, item.label.length * 6 + 10));
    case "bar":
      return Math.max(BAR_W, Math.min(48, item.label.length * 6 + 6));
    case "value":
      return 86;
    case "lamp":
      return Math.max(46, Math.min(82, item.label.length * 7 + 16));
  }
}

export function layoutWidgets(
  resolved: readonly ResolvedWidget[],
  h: number,
  scrollX = 0,
): { items: WidgetLayout[]; totalWidth: number } {
  const cy = Math.min(34, h * 0.46);
  const radius = Math.min(22, h * 0.32);
  const barY = 12;
  const barH = Math.max(20, h - 28);
  let x = STRIP_LEFT - scrollX;
  const items: WidgetLayout[] = [];
  for (let i = 0; i < resolved.length; i += 1) {
    const item = resolved[i];
    const w = widgetWidth(item);
    const kind = item.widget.kind;
    let box: Box;
    if (kind === "arc") {
      box = { x, y: Math.max(2, cy - radius - 6), w, h: Math.min(h - 4, radius * 2 + 24) };
    } else if (kind === "bar") {
      box = { x, y: barY, w, h: barH + 14 };
    } else if (kind === "digit") {
      const boxH = item.label ? DIGIT_SIZE + 14 : DIGIT_SIZE + 4;
      box = { x, y: Math.max(4, cy - DIGIT_SIZE / 2 - 2), w, h: Math.min(h - 8, boxH) };
    } else if (kind === "value") {
      box = { x, y: 10, w, h: Math.max(36, h - 20) };
    } else {
      box = { x, y: Math.round((h - LAMP_H) / 2), w, h: LAMP_H };
    }
    items.push({ index: i, box, cx: x + w / 2, cy, radius, barY, barH });
    x += w + WIDGET_GAP;
  }
  const totalWidth = Math.max(0, x + scrollX - WIDGET_GAP + STRIP_LEFT);
  return { items, totalWidth };
}

/** Index of widget under `(x, y)` in CSS pixels, or -1. */
export function widgetAt(
  x: number,
  y: number,
  w: number,
  h: number,
  resolved: readonly ResolvedWidget[],
  scrollX = 0,
): number {
  const { items } = layoutWidgets(resolved, h, scrollX);
  for (const item of items) {
    const b = item.box;
    if (b.x + b.w < 0 || b.x > w) continue;
    if (x >= b.x - 2 && x <= b.x + b.w + 2 && y >= b.y - 2 && y <= b.y + b.h + 2) {
      return item.index;
    }
  }
  return -1;
}

/** Insertion index (`0..resolved.length`) for a pointer at CSS X coordinate `x`. */
export function dropIndexAt(
  x: number,
  resolved: readonly ResolvedWidget[],
  h: number,
  scrollX = 0,
): number {
  const { items } = layoutWidgets(resolved, h, scrollX);
  for (let i = 0; i < items.length; i += 1) {
    if (x < items[i].cx) return i;
  }
  return items.length;
}

export interface ClusterInteraction {
  hoverIndex?: number;
  selectedIndex?: number;
  dragIndex?: number;
  dropIndex?: number | null;
  scrollX?: number;
}

export function drawWidgets(
  canvas: HTMLCanvasElement,
  resolved: readonly ResolvedWidget[] | null,
  interaction?: ClusterInteraction,
): void {
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

  if (!resolved || resolved.length === 0) {
    ctx.fillStyle = TEXT_MUTED;
    ctx.font = `11px ${SANS}`;
    ctx.textAlign = "left";
    ctx.textBaseline = "middle";
    ctx.fillText("Click + Add or drag a signal here to build your cluster", STRIP_LEFT, h / 2);
    return;
  }

  const scrollX = interaction?.scrollX ?? 0;
  const { items, totalWidth } = layoutWidgets(resolved, h, scrollX);
  const hoverIdx = interaction?.hoverIndex ?? -1;
  const selectedIdx = interaction?.selectedIndex ?? -1;
  const dragIdx = interaction?.dragIndex ?? -1;
  const dropIdx = interaction?.dropIndex ?? null;

  for (let i = 0; i < resolved.length; i += 1) {
    const r = resolved[i];
    const layout = items[i];
    const b = layout.box;
    if (b.x + b.w < -10 || b.x > w + 10) continue;

    ctx.save();
    if (i === dragIdx) ctx.globalAlpha = 0.35;

    if (i === hoverIdx || i === selectedIdx) {
      ctx.fillStyle = i === selectedIdx ? "rgba(230, 162, 60, 0.10)" : "rgba(255, 255, 255, 0.04)";
      ctx.strokeStyle = i === selectedIdx ? "rgba(230, 162, 60, 0.65)" : "rgba(139, 154, 166, 0.35)";
      ctx.lineWidth = 1;
      ctx.beginPath();
      roundRect(ctx, b.x - 3, 4, b.w + 6, h - 8, 3);
      ctx.fill();
      ctx.stroke();
    }

    switch (r.widget.kind) {
      case "arc":
        arcGauge(
          ctx,
          layout.cx,
          layout.cy,
          layout.radius,
          r.value,
          r.min,
          r.max,
          r.color,
          r.label,
          r.decimals,
          r.value == null,
          r.warnFrom,
        );
        break;
      case "digit":
        digitTile(ctx, layout.cx, layout.cy, r.value, r.enumLabel, r.label, r.color, r.assigned);
        break;
      case "bar": {
        const bx = layout.cx - BAR_W / 2;
        const hot = r.warnFrom != null && r.value != null && r.value >= r.warnFrom;
        barGauge(ctx, bx, layout.barY, BAR_W, layout.barH, r.value, r.min, r.max, hot ? RED : r.color, r.label, r.decimals, hot);
        break;
      }
      case "value":
        valueCard(ctx, b, r.value, r.enumLabel, r.label, r.unit, r.decimals, r.color);
        break;
      case "lamp":
        lampChip(ctx, b, r.label, r.lit, r.color);
        break;
    }
    ctx.restore();
  }

  // Insertion bar during drag-and-drop reordering or signal drop
  if (dropIdx != null && dropIdx >= 0 && dropIdx <= items.length) {
    let lineX: number;
    if (items.length === 0) lineX = STRIP_LEFT;
    else if (dropIdx === 0) lineX = items[0].box.x - WIDGET_GAP / 2;
    else if (dropIdx >= items.length) lineX = items[items.length - 1].box.x + items[items.length - 1].box.w + WIDGET_GAP / 2;
    else lineX = (items[dropIdx - 1].box.x + items[dropIdx - 1].box.w + items[dropIdx].box.x) / 2;
    ctx.strokeStyle = AMBER;
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(lineX, 8);
    ctx.lineTo(lineX, h - 8);
    ctx.stroke();
  }

  // Subtle hint when nothing matches a custom DBC yet
  const anything = resolved.some((r) => r.signal != null);
  const tailX = totalWidth - scrollX + 12;
  if (!anything && tailX < w - 160) {
    ctx.fillStyle = TEXT_MUTED;
    ctx.font = `11px ${SANS}`;
    ctx.textAlign = "left";
    ctx.textBaseline = "middle";
    ctx.fillText("Click a gadget to bind or customize · Drag to reorder", tailX, h / 2);
  }
}

/** Legacy fixed-slot geometry (kept for `slotAt` backward compatibility). */
const LEGACY_ANCHOR = { speed: 58, rpm: 158, gear: 108, bar1: 214, bar2: 254, bar3: 294 } as const;
const LEGACY_LAMP_X = 360;
const LEGACY_LAMP_PITCH = 52;
const LEGACY_LAMP_W = 46;
const LEGACY_LAMP_H = 22;
const LEGACY_BAR_W = 28;
const LEGACY_DIGIT = 22;

function legacyBoxes(h: number): Record<SlotId, Box> {
  const cy = Math.min(34, h * 0.46);
  const radius = Math.min(22, h * 0.32);
  const barY = 14;
  const barH = Math.max(20, h - 28);
  const dial = (cx: number): Box => ({ x: cx - radius - 6, y: cy - radius - 6, w: (radius + 6) * 2, h: radius * 2 + 22 });
  const bar = (x: number): Box => ({ x, y: barY, w: LEGACY_BAR_W, h: barH + 12 });
  const lamp = (index: number): Box => ({ x: LEGACY_LAMP_X + index * LEGACY_LAMP_PITCH, y: h / 2 - 11, w: LEGACY_LAMP_W, h: LEGACY_LAMP_H });
  return {
    speed: dial(LEGACY_ANCHOR.speed),
    rpm: dial(LEGACY_ANCHOR.rpm),
    gear: { x: LEGACY_ANCHOR.gear - LEGACY_DIGIT / 2, y: cy - 12, w: LEGACY_DIGIT, h: LEGACY_DIGIT },
    bar1: bar(LEGACY_ANCHOR.bar1),
    bar2: bar(LEGACY_ANCHOR.bar2),
    bar3: bar(LEGACY_ANCHOR.bar3),
    lamp1: lamp(0),
    lamp2: lamp(1),
    lamp3: lamp(2),
    lamp4: lamp(3),
    lamp5: lamp(4),
    lamp6: lamp(5),
  };
}

/** Legacy hit-test helper (retained for existing unit tests). */
export function slotAt(x: number, y: number, w: number, h: number): SlotId | null {
  const boxes = legacyBoxes(h);
  for (const slot of SLOTS) {
    const box = boxes[slot.id];
    if (box.x + box.w > w) continue;
    if (x >= box.x && x <= box.x + box.w && y >= box.y && y <= box.y + box.h) return slot.id;
  }
  return null;
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
  if (redFrom != null && redFrom >= min && redFrom <= max) {
    const ru = (redFrom - min) / (max - min);
    ctx.strokeStyle = "rgba(255, 77, 46, 0.72)";
    ctx.beginPath();
    ctx.arc(cx, cy, radius, start + sweep * ru, start + sweep);
    ctx.stroke();
  }
  const shown = value ?? min;
  const u = Math.min(1, Math.max(0, (shown - min) / (max - min)));
  const hot = !empty && redFrom != null && value != null && value >= redFrom;
  if (hot) {
    ctx.save();
    ctx.strokeStyle = "rgba(255, 90, 69, 0.32)";
    ctx.lineWidth = 7;
    ctx.beginPath();
    ctx.arc(cx, cy, radius, start, start + sweep * u);
    ctx.stroke();
    ctx.restore();
  }
  ctx.strokeStyle = empty ? "rgba(180, 200, 210, 0.25)" : hot ? RED : color;
  ctx.beginPath();
  ctx.arc(cx, cy, radius, start, start + sweep * u);
  ctx.stroke();
  const ang = start + sweep * u;
  ctx.strokeStyle = empty ? "rgba(231,238,243,0.35)" : hot ? RED : TEXT;
  ctx.lineWidth = 1.2;
  ctx.beginPath();
  ctx.moveTo(cx + Math.cos(ang) * 6, cy + Math.sin(ang) * 6);
  ctx.lineTo(cx + Math.cos(ang) * (radius - 1), cy + Math.sin(ang) * (radius - 1));
  ctx.stroke();
  ctx.fillStyle = empty ? TEXT_FAINT : hot ? "#ff9e8f" : TEXT;
  ctx.font = `500 12px ${MONO}`;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(empty || value == null ? "—" : value.toFixed(decimals), cx, cy + 8);
  ctx.fillStyle = TEXT_MUTED;
  ctx.font = `9px ${MONO}`;
  ctx.fillText(unit.slice(0, 10), cx, cy + radius + 8);
}

function digitTile(
  ctx: CanvasRenderingContext2D,
  cx: number,
  cy: number,
  value: number | null,
  enumLabel: string | null,
  label: string,
  color: string,
  assigned: boolean,
): void {
  const display =
    value == null
      ? "–"
      : enumLabel
        ? enumLabel.slice(0, 5).toUpperCase()
        : !assigned && value < 0.5
          ? "N"
          : String(Math.round(value)).slice(0, 4);
  const w = Math.max(DIGIT_SIZE, Math.min(54, display.length * 8 + 12));
  const y = cy - DIGIT_SIZE / 2;
  ctx.fillStyle = WELL;
  ctx.strokeStyle = EDGE;
  ctx.lineWidth = 1;
  ctx.beginPath();
  roundRect(ctx, cx - w / 2, y, w, DIGIT_SIZE, 2);
  ctx.fill();
  ctx.stroke();
  ctx.fillStyle = value == null ? TEXT_FAINT : color;
  ctx.font = display.length > 2 ? `600 10px ${MONO}` : `600 13px ${MONO}`;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(display, cx, cy);
  if (label) {
    ctx.fillStyle = TEXT_MUTED;
    ctx.font = `9px ${MONO}`;
    ctx.textBaseline = "top";
    ctx.fillText(label.slice(0, 8), cx, y + DIGIT_SIZE + 3);
  }
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
  hot = false,
): void {
  ctx.fillStyle = hot ? "rgba(255, 90, 69, 0.14)" : WELL;
  ctx.strokeStyle = hot ? RED : RULE;
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
  ctx.fillText(label.slice(0, 7), x + width / 2, y + height + 3);
}

function valueCard(
  ctx: CanvasRenderingContext2D,
  box: Box,
  value: number | null,
  enumLabel: string | null,
  label: string,
  unit: string,
  decimals: number,
  color: string,
): void {
  ctx.fillStyle = WELL;
  ctx.strokeStyle = RULE;
  ctx.lineWidth = 1;
  ctx.beginPath();
  roundRect(ctx, box.x, box.y, box.w, box.h, 3);
  ctx.fill();
  ctx.stroke();

  // Accent bar on the left edge
  ctx.fillStyle = value == null ? EDGE : color;
  ctx.fillRect(box.x + 1, box.y + 4, 2.5, box.h - 8);

  // Top label
  ctx.fillStyle = TEXT_MUTED;
  ctx.font = `600 9px ${SANS}`;
  ctx.textAlign = "left";
  ctx.textBaseline = "top";
  ctx.fillText(label.slice(0, 12), box.x + 8, box.y + 5);

  // Main value
  const text =
    value == null
      ? "—"
      : enumLabel
        ? enumLabel.slice(0, 9)
        : value.toFixed(decimals);
  ctx.fillStyle = value == null ? TEXT_FAINT : TEXT;
  ctx.font = text.length > 7 ? `500 11px ${MONO}` : `600 13px ${MONO}`;
  ctx.textAlign = "left";
  ctx.textBaseline = "middle";
  ctx.fillText(text, box.x + 8, box.y + box.h * 0.56);

  // Unit at bottom-right
  if (unit && !enumLabel) {
    ctx.fillStyle = TEXT_MUTED;
    ctx.font = `9px ${MONO}`;
    ctx.textAlign = "right";
    ctx.textBaseline = "bottom";
    ctx.fillText(unit.slice(0, 6), box.x + box.w - 6, box.y + box.h - 4);
  }
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
  ctx.fillText(label.slice(0, 10), box.x + box.w / 2, box.y + box.h / 2);
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
