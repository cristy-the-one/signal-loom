import { fitCanvas } from "./canvas";
import { formatUs, tickStep } from "./format";
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
}

export function drawPlot(
  canvas: HTMLCanvasElement,
  view: { t0: number; t1: number },
  playhead: number,
  traces: Trace[],
): void {
  const fitted = fitCanvas(canvas);
  if (!fitted) return;
  const { ctx, w, h } = fitted;
  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = "#090c0f";
  ctx.fillRect(0, 0, w, h);

  const left = 8;
  const right = 12;
  const top = 16;
  const bottom = 22;
  const plotW = Math.max(1, w - left - right);
  const plotH = Math.max(1, h - top - bottom);
  const span = Math.max(1, view.t1 - view.t0);

  ctx.save();
  ctx.beginPath();
  ctx.rect(left, top, plotW, plotH);
  ctx.clip();

  ctx.strokeStyle = "rgba(180, 200, 210, 0.08)";
  ctx.lineWidth = 1;
  for (let i = 1; i < 4; i += 1) {
    const y = top + (plotH * i) / 4;
    ctx.beginPath();
    ctx.moveTo(left, y);
    ctx.lineTo(left + plotW, y);
    ctx.stroke();
  }

  const step = tickStep(span);
  const first = Math.ceil(view.t0 / step) * step;
  ctx.strokeStyle = "rgba(180, 200, 210, 0.06)";
  for (let t = first; t <= view.t1; t += step) {
    const x = left + ((t - view.t0) / span) * plotW;
    ctx.beginPath();
    ctx.moveTo(x, top);
    ctx.lineTo(x, top + plotH);
    ctx.stroke();
  }

  for (const trace of traces) {
    drawTrace(ctx, trace, view, left, top, plotW, plotH);
  }

  if (playhead >= view.t0 && playhead <= view.t1) {
    const x = left + ((playhead - view.t0) / span) * plotW;
    ctx.strokeStyle = "#ff4d2e";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(x, top);
    ctx.lineTo(x, top + plotH);
    ctx.stroke();
  }
  ctx.restore();

  ctx.fillStyle = "#8b9aa6";
  ctx.font = "11px 'IBM Plex Mono', ui-monospace, monospace";
  ctx.textAlign = "left";
  ctx.textBaseline = "top";
  for (let t = first; t <= view.t1; t += step) {
    const x = left + ((t - view.t0) / span) * plotW;
    ctx.fillText(formatUs(t), x + 4, top + plotH + 4);
  }
}

function drawTrace(
  ctx: CanvasRenderingContext2D,
  trace: Trace,
  view: { t0: number; t1: number },
  left: number,
  top: number,
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
  const yOf = (v: number) => top + (1 - (v - lo) / (hi - lo)) * plotH;
  const xOf = (t: number) => left + ((t - view.t0) / span) * plotW;

  ctx.beginPath();
  ctx.strokeStyle = trace.color;
  ctx.lineWidth = 1.5;
  ctx.lineJoin = "round";
  ctx.lineCap = "round";
  const x0 = xOf(Math.max(points[0].t, view.t0));
  ctx.moveTo(x0, yOf(points[0].v));
  for (let i = 1; i < points.length; i += 1) {
    const prev = points[i - 1];
    const cur = points[i];
    ctx.lineTo(xOf(cur.t), yOf(prev.v));
    ctx.lineTo(xOf(cur.t), yOf(cur.v));
  }
  const last = points[points.length - 1];
  ctx.lineTo(xOf(view.t1), yOf(last.v));
  ctx.stroke();
}
