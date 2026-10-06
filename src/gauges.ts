import { fitCanvas } from "./canvas";
import { formatValue } from "./format";
import type { BusLoad, ValueRead } from "./types";

export interface ClusterReadings {
  speed: number | null;
  rpm: number | null;
  coolant: number | null;
  oil: number | null;
  soc: number | null;
  fuel: number | null;
  gear: number | null;
  mil: boolean;
  abs: boolean;
  left: boolean;
  right: boolean;
  door: boolean;
  esc: boolean;
}

function num(values: ValueRead[], name: string): number | null {
  const hit = values.find((item) => item.name === name);
  return hit ? hit.value : null;
}

function on(values: ValueRead[], name: string): boolean {
  const value = num(values, name);
  return value != null && value > 0.5;
}

export function readingsFrom(values: ValueRead[]): ClusterReadings {
  return {
    speed: num(values, "VehicleSpeed"),
    rpm: num(values, "EngineRPM") ?? num(values, "DisplayedRPM"),
    coolant: num(values, "CoolantTemp"),
    oil: num(values, "OilTemp"),
    soc: num(values, "Soc"),
    fuel: num(values, "FuelLevel"),
    gear: num(values, "Gear") ?? num(values, "GearActual"),
    mil: on(values, "MilLamp") || on(values, "TelltaleMil"),
    abs: on(values, "AbsActive") || on(values, "TelltaleAbs"),
    left: on(values, "TurnLeft") || on(values, "TelltaleLeft"),
    right: on(values, "TurnRight") || on(values, "TelltaleRight"),
    door: on(values, "DoorFL"),
    esc: on(values, "EscActive"),
  };
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

  const data = readings ?? {
    speed: null,
    rpm: null,
    coolant: null,
    oil: null,
    soc: null,
    fuel: null,
    gear: null,
    mil: false,
    abs: false,
    left: false,
    right: false,
    door: false,
    esc: false,
  };
  const cy = Math.min(34, h * 0.46);
  const radius = Math.min(22, h * 0.32);
  arcGauge(ctx, 58, cy, radius, data.speed, 0, 320, "#f2e394", "km/h", data.speed == null);
  arcGauge(ctx, 158, cy, radius, data.rpm, 0, 9000, "#3ec6ff", "rpm", data.rpm == null, 7500);
  gearDigit(ctx, 108, cy, data.gear);

  const barX = 214;
  const barY = 14;
  const barH = Math.max(20, h - 28);
  barGauge(ctx, barX, barY, 28, barH, data.coolant, 40, 130, "#ff7a59", "CLT");
  barGauge(ctx, barX + 40, barY, 28, barH, data.oil, 40, 150, "#e6a23c", "OIL");
  const energy = data.soc ?? data.fuel;
  barGauge(ctx, barX + 80, barY, 28, barH, energy, 0, 100, "#c9a0ff", data.soc != null ? "SOC" : "FUEL");

  const lamps: { label: string; on: boolean; color: string }[] = [
    { label: "MIL", on: data.mil, color: "#ffb020" },
    { label: "ABS", on: data.abs, color: "#ff5a45" },
    { label: "ESC", on: data.esc, color: "#e6a23c" },
    { label: "L", on: data.left, color: "#7ddea5" },
    { label: "R", on: data.right, color: "#7ddea5" },
    { label: "DOOR", on: data.door, color: "#7ddea5" },
  ];
  lamps.forEach((lamp, index) => {
    const x = 360 + index * 52;
    if (x > w - 46) return;
    lampChip(ctx, x, h / 2 - 11, lamp.label, lamp.on, lamp.color);
  });
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
  ctx.font = "600 13px 'IBM Plex Sans', sans-serif";
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(empty || value == null ? "—" : formatValue(value), cx, cy + 8);
  ctx.fillStyle = "#8b9aa6";
  ctx.font = "9px 'IBM Plex Mono', ui-monospace, monospace";
  ctx.fillText(unit, cx, cy + radius + 8);
}

function gearDigit(ctx: CanvasRenderingContext2D, x: number, y: number, gear: number | null): void {
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
  const label = gear == null ? "–" : gear < 0.5 ? "N" : String(Math.round(gear));
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
  ctx.fillText(value == null ? "—" : formatValue(value), x + width / 2, y + 3);
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
  const rate = load ? `${load.rate.toFixed(0)} f/s` : "— f/s";
  const pct = load ? `${(load.load * 100).toFixed(1)}% of 500 kbit/s` : "bus idle";
  const frames = load ? `${load.frames} fr / 1 s` : "";
  const label = `${rate}   ${pct}   ${frames}`.trim();
  ctx.fillText(label, pad, h / 2);
  const textW = ctx.measureText(label).width;
  const barX = pad + textW + 12;
  const barW = Math.max(24, w - barX - pad);
  const barY = Math.max(3, (h - 6) / 2);
  ctx.fillStyle = "#1a2229";
  ctx.fillRect(barX, barY, barW, 6);
  const fraction = load ? Math.min(1, Math.max(0, load.load)) : 0;
  ctx.fillStyle = fraction > 0.7 ? "#ff5a45" : fraction > 0.4 ? "#e6a23c" : "#3ec6ff";
  ctx.fillRect(barX, barY, barW * fraction, 6);
}
