import { fitCanvas } from "./canvas";
import { clamp, formatUs, formatValue, tickStep } from "./format";
import type { Point } from "./types";

export const TRACE_COLORS = [
  "#3ec6ff",
  "#e6a23c",
  "#7ddea5",
  "#ff7a59",
  "#c9a0ff",
  "#f2e394",
  "#6ee0d0",
  "#ff8cc6",
];

export interface Trace {
  name: string;
  color: string;
  points: Point[];
  min: number;
  max: number;
  dashed?: boolean;
}

export interface PlotOverlay {
  hoverT: number | null;
  cursorA: number | null;
  cursorB: number | null;
}

const PAD = { left: 8, right: 12, top: 16, bottom: 22 };

export function drawPlot(
  canvas: HTMLCanvasElement,
  view: { t0: number; t1: number },
  playhead: number,
  traces: Trace[],
  overlay: PlotOverlay,
): void {
  const fitted = fitCanvas(canvas);
  if (!fitted) return;
  const { ctx, w, h } = fitted;
  const bg = ctx.createLinearGradient(0, 0, 0, h);
  bg.addColorStop(0, "#12181e");
  bg.addColorStop(1, "#07090c");
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, w, h);

  const plotW = Math.max(1, w - PAD.left - PAD.right);
  const plotH = Math.max(1, h - PAD.top - PAD.bottom);
  const span = Math.max(1, view.t1 - view.t0);
  const xOf = (t: number) => PAD.left + ((t - view.t0) / span) * plotW;

  ctx.save();
  ctx.beginPath();
  ctx.rect(PAD.left, PAD.top, plotW, plotH);
  ctx.clip();

  ctx.strokeStyle = "rgba(180, 200, 210, 0.07)";
  ctx.lineWidth = 1;
  for (let i = 1; i < 4; i += 1) {
    const y = PAD.top + (plotH * i) / 4;
    ctx.beginPath();
    ctx.moveTo(PAD.left, y + 0.5);
    ctx.lineTo(PAD.left + plotW, y + 0.5);
    ctx.stroke();
  }

  const step = tickStep(span);
  const first = Math.ceil(view.t0 / step) * step;
  ctx.strokeStyle = "rgba(180, 200, 210, 0.045)";
  for (let t = first; t <= view.t1; t += step) {
    const x = xOf(t);
    ctx.beginPath();
    ctx.moveTo(x + 0.5, PAD.top);
    ctx.lineTo(x + 0.5, PAD.top + plotH);
    ctx.stroke();
  }

  for (const trace of traces) {
    drawTrace(ctx, trace, view, plotW, plotH);
  }

  drawCursor(ctx, overlay.cursorA, view, plotW, plotH, "#f2e394");
  drawCursor(ctx, overlay.cursorB, view, plotW, plotH, "#7ddea5");

  if (overlay.hoverT != null && overlay.hoverT >= view.t0 && overlay.hoverT <= view.t1) {
    const x = xOf(overlay.hoverT);
    ctx.strokeStyle = "rgba(231, 238, 243, 0.45)";
    ctx.lineWidth = 1;
    ctx.setLineDash([2, 3]);
    ctx.beginPath();
    ctx.moveTo(x + 0.5, PAD.top);
    ctx.lineTo(x + 0.5, PAD.top + plotH);
    ctx.stroke();
    ctx.setLineDash([]);
    for (const trace of traces) {
      const value = heldValue(trace.points, overlay.hoverT);
      if (value == null) continue;
      const range = trace.max - trace.min;
      const pad = range === 0 ? 1 : range * 0.08;
      const lo = trace.min - pad;
      const hi = trace.max + pad;
      const y = PAD.top + (1 - (value - lo) / (hi - lo)) * plotH;
      ctx.fillStyle = trace.color;
      ctx.beginPath();
      ctx.arc(x, y, 2.4, 0, Math.PI * 2);
      ctx.fill();
    }
  }

  if (playhead >= view.t0 && playhead <= view.t1) {
    const x = xOf(playhead);
    ctx.strokeStyle = "rgba(255, 77, 46, 0.35)";
    ctx.lineWidth = 3;
    ctx.beginPath();
    ctx.moveTo(x, PAD.top);
    ctx.lineTo(x, PAD.top + plotH);
    ctx.stroke();
    ctx.strokeStyle = "#ff4d2e";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(x + 0.5, PAD.top);
    ctx.lineTo(x + 0.5, PAD.top + plotH);
    ctx.stroke();
  }
  ctx.restore();

  ctx.fillStyle = "#8b9aa6";
  ctx.font = "11px 'IBM Plex Mono', ui-monospace, monospace";
  ctx.textAlign = "left";
  ctx.textBaseline = "top";
  for (let t = first; t <= view.t1; t += step) {
    const x = xOf(t);
    ctx.fillText(formatUs(t), x + 4, PAD.top + plotH + 4);
  }
}

export function heldValue(points: Point[], t: number): number | null {
  let value: number | null = null;
  for (const point of points) {
    if (point.t > t) break;
    value = point.v;
  }
  return value;
}

export function timeOnPlot(
  canvas: HTMLCanvasElement,
  clientX: number,
  view: { t0: number; t1: number },
): number {
  const rect = canvas.getBoundingClientRect();
  const plotW = Math.max(1, rect.width - PAD.left - PAD.right);
  const u = (clientX - rect.left - PAD.left) / plotW;
  return view.t0 + clamp(u, 0, 1) * (view.t1 - view.t0);
}

export function formatHover(name: string, value: number | null, unit: string): string {
  if (value == null) return name;
  return unit ? `${name}  ${formatValue(value)} ${unit}` : `${name}  ${formatValue(value)}`;
}

function drawCursor(
  ctx: CanvasRenderingContext2D,
  t: number | null,
  view: { t0: number; t1: number },
  plotW: number,
  plotH: number,
  color: string,
): void {
  if (t == null || t < view.t0 || t > view.t1) return;
  const span = Math.max(1, view.t1 - view.t0);
  const x = PAD.left + ((t - view.t0) / span) * plotW;
  ctx.strokeStyle = color;
  ctx.lineWidth = 1;
  ctx.setLineDash([4, 3]);
  ctx.beginPath();
  ctx.moveTo(x + 0.5, PAD.top);
  ctx.lineTo(x + 0.5, PAD.top + plotH);
  ctx.stroke();
  ctx.setLineDash([]);
}

function drawTrace(
  ctx: CanvasRenderingContext2D,
  trace: Trace,
  view: { t0: number; t1: number },
  plotW: number,
  plotH: number,
): void {
  const points = trace.points;
  if (points.length === 0) return;
  const span = Math.max(1, view.t1 - view.t0);
  const range = trace.max - trace.min;
  const pad = range === 0 ? 1 : range * 0.08;
  const lo = trace.min - pad;
  const hi = trace.max + pad;
  const yOf = (v: number) => PAD.top + (1 - (v - lo) / (hi - lo)) * plotH;
  const xOf = (t: number) => PAD.left + ((t - view.t0) / span) * plotW;

  ctx.beginPath();
  ctx.strokeStyle = trace.color;
  ctx.globalAlpha = trace.dashed ? 0.8 : 1;
  ctx.lineWidth = trace.dashed ? 1.15 : 1.5;
  ctx.lineJoin = "round";
  ctx.lineCap = "round";
  ctx.setLineDash(trace.dashed ? [5, 3] : []);
  ctx.moveTo(xOf(Math.max(points[0].t, view.t0)), yOf(points[0].v));
  for (let i = 1; i < points.length; i += 1) {
    const prev = points[i - 1];
    const cur = points[i];
    ctx.lineTo(xOf(cur.t), yOf(prev.v));
    ctx.lineTo(xOf(cur.t), yOf(cur.v));
  }
  const last = points[points.length - 1];
  ctx.lineTo(xOf(view.t1), yOf(last.v));
  ctx.stroke();
  ctx.setLineDash([]);
  ctx.globalAlpha = 1;
}
