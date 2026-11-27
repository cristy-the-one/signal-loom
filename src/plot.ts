import { eachTick, fitCanvas } from "./canvas";
import { clamp, formatReading, formatTimeForSpan, formatValue, type Readout } from "./format";
import { AMBER, CORAL, CYAN, GREEN, MONO, PINK, PLAYHEAD, SURFACE, TEAL, TEXT_MUTED, VIOLET, YELLOW } from "./theme";
import type { Point } from "./types";

export const TRACE_COLORS = [CYAN, AMBER, GREEN, CORAL, VIOLET, YELLOW, TEAL, PINK];

export interface Trace {
  name: string;
  color: string;
  points: Point[];
  min: number;
  max: number;
  dashed?: boolean;
  /** Where the fetched data ends; the trace stops here rather than holding its last value to the edge. */
  endT?: number;
}

export interface PlotLane {
  title: string;
  traces: Trace[];
  sharedScale: boolean;
  scaleMin: number;
  scaleMax: number;
}

export interface PlotOverlay {
  hoverT: number | null;
  cursorA: number | null;
  cursorB: number | null;
  boxZoom?: { t0: number; t1: number } | null;
}

const PAD = { left: 8, right: 12, top: 12, bottom: 22 };

function yForTrace(value: number, trace: Trace, top: number, height: number): number {
  const range = trace.max - trace.min;
  const pad = range === 0 ? 1 : range * 0.08;
  const lo = trace.min - pad;
  const hi = trace.max + pad;
  return top + (1 - (value - lo) / (hi - lo)) * height;
}

export function drawPlot(
  canvas: HTMLCanvasElement,
  view: { t0: number; t1: number },
  playhead: number,
  traces: Trace[],
  overlay: PlotOverlay,
  lanes?: PlotLane[],
): void {
  const fitted = fitCanvas(canvas);
  if (!fitted) return;
  const { ctx, w, h } = fitted;
  const bg = ctx.createLinearGradient(0, 0, 0, h);
  bg.addColorStop(0, SURFACE);
  bg.addColorStop(1, "#07090c");
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, w, h);

  const plotW = Math.max(1, w - PAD.left - PAD.right);
  const plotH = Math.max(1, h - PAD.top - PAD.bottom);
  const span = Math.max(1, view.t1 - view.t0);
  const xOf = (t: number) => PAD.left + ((t - view.t0) / span) * plotW;

  const effectiveLanes: PlotLane[] =
    lanes && lanes.length > 0
      ? lanes
      : [
          {
            title: "",
            traces,
            sharedScale: traces.length === 1,
            scaleMin: traces[0]?.min ?? 0,
            scaleMax: traces[0]?.max ?? 1,
          },
        ];
  const multiLane = effectiveLanes.length > 1;
  const laneGap = multiLane
    ? Math.min(6, Math.max(2, Math.floor(plotH / (effectiveLanes.length * 14))))
    : 0;
  const laneH = Math.max(
    8,
    (plotH - laneGap * (effectiveLanes.length - 1)) / effectiveLanes.length,
  );

  ctx.save();
  ctx.beginPath();
  ctx.rect(PAD.left, PAD.top, plotW, plotH);
  ctx.clip();

  if (!multiLane) {
    ctx.strokeStyle = "rgba(180, 200, 210, 0.07)";
    ctx.lineWidth = 1;
    for (let i = 1; i < 4; i += 1) {
      const y = PAD.top + (plotH * i) / 4;
      ctx.beginPath();
      ctx.moveTo(PAD.left, y + 0.5);
      ctx.lineTo(PAD.left + plotW, y + 0.5);
      ctx.stroke();
    }
  } else {
    for (let k = 0; k < effectiveLanes.length; k += 1) {
      const laneTop = PAD.top + k * (laneH + laneGap);
      if (k % 2 === 1) {
        ctx.fillStyle = "rgba(255, 255, 255, 0.015)";
        ctx.fillRect(PAD.left, laneTop, plotW, laneH);
      }
      if (k > 0) {
        const sepY = Math.round(laneTop - laneGap / 2) + 0.5;
        ctx.strokeStyle = "rgba(150, 175, 195, 0.2)";
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(PAD.left, sepY);
        ctx.lineTo(PAD.left + plotW, sepY);
        ctx.stroke();
      }
      const midY = Math.round(laneTop + laneH / 2) + 0.5;
      ctx.strokeStyle = "rgba(180, 200, 210, 0.05)";
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(PAD.left, midY);
      ctx.lineTo(PAD.left + plotW, midY);
      ctx.stroke();
    }
  }

  ctx.strokeStyle = "rgba(180, 200, 210, 0.045)";
  ctx.lineWidth = 1;
  eachTick(view.t0, view.t1, (t) => {
    const x = xOf(t);
    ctx.beginPath();
    ctx.moveTo(x + 0.5, PAD.top);
    ctx.lineTo(x + 0.5, PAD.top + plotH);
    ctx.stroke();
  });

  for (let k = 0; k < effectiveLanes.length; k += 1) {
    const lane = effectiveLanes[k];
    const laneTop = PAD.top + k * (laneH + laneGap);
    ctx.save();
    ctx.beginPath();
    ctx.rect(PAD.left, laneTop, plotW, laneH);
    ctx.clip();
    for (const trace of lane.traces) {
      drawTrace(ctx, trace, view, plotW, laneTop, laneH);
    }
    if (multiLane && lane.title && laneH >= 20) {
      ctx.font = `10px ${MONO}`;
      ctx.textAlign = "left";
      ctx.textBaseline = "top";
      ctx.fillStyle = lane.traces.length === 1 ? lane.traces[0].color : "rgba(231, 238, 243, 0.72)";
      ctx.fillText(lane.title, PAD.left + 5, laneTop + 3);
    }
    if (lane.sharedScale && lane.traces.length > 0 && laneH >= 32) {
      ctx.font = `10px ${MONO}`;
      ctx.textAlign = "right";
      ctx.fillStyle = "rgba(139, 154, 166, 0.76)";
      ctx.textBaseline = "top";
      ctx.fillText(formatValue(lane.scaleMax), PAD.left + plotW - 4, laneTop + 3);
      ctx.textBaseline = "bottom";
      ctx.fillText(formatValue(lane.scaleMin), PAD.left + plotW - 4, laneTop + laneH - 2);
    }
    ctx.restore();
  }

  if (overlay.boxZoom) {
    const bx0 = xOf(Math.min(overlay.boxZoom.t0, overlay.boxZoom.t1));
    const bx1 = xOf(Math.max(overlay.boxZoom.t0, overlay.boxZoom.t1));
    const bw = Math.max(1, bx1 - bx0);
    ctx.fillStyle = "rgba(62, 198, 255, 0.14)";
    ctx.fillRect(bx0, PAD.top, bw, plotH);
    ctx.strokeStyle = "rgba(62, 198, 255, 0.85)";
    ctx.lineWidth = 1;
    ctx.strokeRect(bx0 + 0.5, PAD.top + 0.5, Math.max(1, bw - 1), plotH - 1);
  }

  drawCursor(ctx, overlay.cursorA, view, plotW, plotH, YELLOW);
  drawCursor(ctx, overlay.cursorB, view, plotW, plotH, GREEN);

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
    for (let k = 0; k < effectiveLanes.length; k += 1) {
      const lane = effectiveLanes[k];
      const laneTop = PAD.top + k * (laneH + laneGap);
      for (const trace of lane.traces) {
        const value = heldValue(trace.points, overlay.hoverT);
        if (value == null) continue;
        const y = yForTrace(value, trace, laneTop, laneH);
        ctx.fillStyle = trace.color;
        ctx.beginPath();
        ctx.arc(x, y, 2.4, 0, Math.PI * 2);
        ctx.fill();
      }
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
    ctx.strokeStyle = PLAYHEAD;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(x + 0.5, PAD.top);
    ctx.lineTo(x + 0.5, PAD.top + plotH);
    ctx.stroke();
  }
  ctx.restore();

  ctx.fillStyle = TEXT_MUTED;
  ctx.font = `11px ${MONO}`;
  ctx.textAlign = "left";
  ctx.textBaseline = "top";
  eachTick(view.t0, view.t1, (t) => {
    const x = xOf(t);
    ctx.fillText(formatTimeForSpan(t, span), x + 4, PAD.top + plotH + 4);
  });
}

export function heldValue(points: Point[], t: number): number | null {
  let lo = 0;
  let hi = points.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (points[mid].t <= t) lo = mid + 1;
    else hi = mid;
  }
  return lo > 0 ? points[lo - 1].v : null;
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

export function formatHover(
  name: string,
  value: number | null,
  unit: string,
  readout: Readout,
  enumLabel?: string | null,
): string {
  const reading = value == null ? "—".padStart(readout.width) : formatReading(value, readout);
  const tail = [unit, enumLabel ? `(${enumLabel})` : ""].filter(Boolean).join(" ");
  return tail ? `${name}  ${reading} ${tail}` : `${name}  ${reading}`;
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
  laneTop: number,
  laneH: number,
): void {
  const points = trace.points;
  if (points.length === 0) return;
  const span = Math.max(1, view.t1 - view.t0);
  const yOf = (v: number) => yForTrace(v, trace, laneTop, laneH);
  const xOf = (t: number) => PAD.left + ((t - view.t0) / span) * plotW;

  ctx.beginPath();
  ctx.strokeStyle = trace.color;
  ctx.globalAlpha = trace.dashed ? 0.8 : 1;
  ctx.lineWidth = trace.dashed ? 1.15 : 1.5;
  ctx.lineJoin = "round";
  ctx.lineCap = "round";
  ctx.setLineDash(trace.dashed ? [5, 3] : []);
  ctx.moveTo(xOf(points[0].t), yOf(points[0].v));
  for (let i = 1; i < points.length; i += 1) {
    const prev = points[i - 1];
    const cur = points[i];
    ctx.lineTo(xOf(cur.t), yOf(prev.v));
    ctx.lineTo(xOf(cur.t), yOf(cur.v));
  }
  const last = points[points.length - 1];
  ctx.lineTo(xOf(Math.max(last.t, Math.min(view.t1, trace.endT ?? view.t1))), yOf(last.v));
  ctx.stroke();
  ctx.setLineDash([]);
  ctx.globalAlpha = 1;
}
