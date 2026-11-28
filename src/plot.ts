import { eachTick, fitCanvas, roundRect } from "./canvas";
import { clamp, formatReading, formatTimeForSpan, formatValue, type Readout } from "./format";
import { isRibbonStyle } from "./lanes";
import {
  AMBER,
  CORAL,
  CYAN,
  GREEN,
  LIME,
  MONO,
  ORANGE,
  PINK,
  PLAYHEAD,
  SANS,
  SKY,
  SURFACE,
  TEAL,
  TEXT,
  TEXT_MUTED,
  VIOLET,
  YELLOW,
} from "./theme";
import type { ConcreteTraceStyle, Point } from "./types";

export const TRACE_COLORS = [CYAN, AMBER, GREEN, CORAL, VIOLET, YELLOW, TEAL, PINK];

/** High-contrast categorical fills for discrete enum/state ribbon bars. */
const STATE_PALETTE = [
  { bg: "#2b6f91", border: "#4ec2f2", text: "#eef7fc" },
  { bg: "#2d7552", border: "#66d19a", text: "#eefcf4" },
  { bg: "#876123", border: "#e6a23c", text: "#fff7e8" },
  { bg: "#5d438c", border: "#b994ff", text: "#f6f0ff" },
  { bg: "#8a4230", border: "#ff7a59", text: "#fff1ed" },
  { bg: "#296e68", border: "#6ee0d0", text: "#ecfdfb" },
  { bg: "#7d3d60", border: "#ff8cc6", text: "#fff0f8" },
  { bg: "#5b6f2a", border: "#b6e36a", text: "#f6fce8" },
];
const STATE_NEUTRAL = { bg: "#1d262f", border: "#354452", text: "#9ab0c0" };

export interface Trace {
  name: string;
  color: string;
  points: Point[];
  min: number;
  max: number;
  dashed?: boolean;
  /** Where the fetched data ends; the trace stops here rather than holding its last value to the edge. */
  endT?: number;
  /** Semantic rendering style for this trace. Defaults to `"step"`. */
  style?: ConcreteTraceStyle;
  /** Optional enum `VAL_` labels used by `"state"` and `"flag"` ribbons. */
  labels?: { value: number; label: string }[];
}

export interface PlotLane {
  title: string;
  traces: Trace[];
  sharedScale: boolean;
  scaleMin: number;
  scaleMax: number;
  compact?: boolean;
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

interface LaneRect {
  lane: PlotLane;
  top: number;
  height: number;
}

/** Allocate vertical pixels across lanes: compact ribbon lanes take a sleek fixed band so analog tracks stay spacious. */
function computeLaneRects(lanes: PlotLane[], plotH: number, laneGap: number): LaneRect[] {
  const n = lanes.length;
  if (n === 0) return [];
  const totalGap = laneGap * Math.max(0, n - 1);
  const avail = Math.max(16, plotH - totalGap);

  const compactCount = lanes.filter((l) => l.compact).length;
  const normalCount = n - compactCount;

  let compactH: number;
  let normalH: number;
  if (compactCount === 0) {
    compactH = 0;
    normalH = Math.max(12, avail / n);
  } else if (normalCount === 0) {
    compactH = Math.max(20, avail / n);
    normalH = 0;
  } else {
    const desiredCompact = Math.min(38, Math.max(26, Math.floor(avail * 0.14)));
    const maxCompactBudget = avail * 0.55;
    compactH = Math.min(desiredCompact, Math.max(20, maxCompactBudget / compactCount));
    normalH = Math.max(24, (avail - compactH * compactCount) / normalCount);
  }

  let y = PAD.top;
  return lanes.map((lane) => {
    const height = lane.compact ? compactH : normalH;
    const rect: LaneRect = { lane, top: y, height };
    y += height + laneGap;
    return rect;
  });
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
            compact: traces.length > 0 && traces.every((t) => isRibbonStyle(t.style ?? "step")),
          },
        ];
  const multiLane = effectiveLanes.length > 1;
  const laneGap = multiLane
    ? Math.min(6, Math.max(2, Math.floor(plotH / (effectiveLanes.length * 14))))
    : 0;
  const rects = computeLaneRects(effectiveLanes, plotH, laneGap);

  ctx.save();
  ctx.beginPath();
  ctx.rect(PAD.left, PAD.top, plotW, plotH);
  ctx.clip();

  if (!multiLane) {
    if (!rects[0]?.lane.compact) {
      ctx.strokeStyle = "rgba(180, 200, 210, 0.07)";
      ctx.lineWidth = 1;
      for (let i = 1; i < 4; i += 1) {
        const y = PAD.top + (plotH * i) / 4;
        ctx.beginPath();
        ctx.moveTo(PAD.left, y + 0.5);
        ctx.lineTo(PAD.left + plotW, y + 0.5);
        ctx.stroke();
      }
    }
  } else {
    for (let k = 0; k < rects.length; k += 1) {
      const { lane, top: laneTop, height: laneH } = rects[k];
      if (lane.compact) {
        ctx.fillStyle = "rgba(18, 25, 32, 0.55)";
        ctx.fillRect(PAD.left, laneTop, plotW, laneH);
      } else if (k % 2 === 1) {
        ctx.fillStyle = "rgba(255, 255, 255, 0.015)";
        ctx.fillRect(PAD.left, laneTop, plotW, laneH);
      }
      if (k > 0) {
        const sepY = Math.round(laneTop - laneGap / 2) + 0.5;
        ctx.strokeStyle = "rgba(150, 175, 195, 0.22)";
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(PAD.left, sepY);
        ctx.lineTo(PAD.left + plotW, sepY);
        ctx.stroke();
      }
      if (!lane.compact) {
        const midY = Math.round(laneTop + laneH / 2) + 0.5;
        ctx.strokeStyle = "rgba(180, 200, 210, 0.05)";
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(PAD.left, midY);
        ctx.lineTo(PAD.left + plotW, midY);
        ctx.stroke();
      }
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

  for (let k = 0; k < rects.length; k += 1) {
    const { lane, top: laneTop, height: laneH } = rects[k];
    ctx.save();
    ctx.beginPath();
    ctx.rect(PAD.left, laneTop, plotW, laneH);
    ctx.clip();

    if (lane.compact && lane.traces.length > 0) {
      // Split compact ribbon traces evenly vertically inside the lane (e.g. Drive A + Drive B compare).
      const headerReserve = multiLane && lane.title && laneH >= 30 ? 11 : 0;
      const bodyTop = laneTop + headerReserve;
      const bodyH = Math.max(12, laneH - headerReserve);
      const count = lane.traces.length;
      const subGap = count > 1 ? 2 : 0;
      const subH = Math.max(8, (bodyH - subGap * (count - 1)) / count);
      lane.traces.forEach((trace, idx) => {
        drawTrace(ctx, trace, view, plotW, bodyTop + idx * (subH + subGap), subH);
      });
    } else {
      for (const trace of lane.traces) {
        drawTrace(ctx, trace, view, plotW, laneTop, laneH);
      }
    }

    if (multiLane && lane.title && laneH >= 20) {
      ctx.font = `10px ${MONO}`;
      ctx.textAlign = "left";
      ctx.textBaseline = "top";
      const labelText = lane.title;
      if (lane.compact) {
        const tw = ctx.measureText(labelText).width;
        ctx.fillStyle = "rgba(9, 12, 15, 0.78)";
        ctx.fillRect(PAD.left + 2, laneTop + 1, tw + 8, 12);
      }
      ctx.fillStyle = lane.traces.length === 1 ? lane.traces[0].color : "rgba(231, 238, 243, 0.78)";
      ctx.fillText(labelText, PAD.left + 5, laneTop + 2);
    }

    if (!lane.compact && lane.sharedScale && lane.traces.length > 0 && laneH >= 32) {
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
    for (const { lane, top: laneTop, height: laneH } of rects) {
      for (const trace of lane.traces) {
        const value = heldValue(trace.points, overlay.hoverT);
        if (value == null) continue;
        const style = trace.style ?? "step";
        const y = isRibbonStyle(style) ? laneTop + laneH / 2 : yForTrace(value, trace, laneTop, laneH);
        ctx.fillStyle = isRibbonStyle(style) ? TEXT : trace.color;
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
  const style = trace.style ?? "step";
  if (style === "state") {
    drawStateRibbon(ctx, trace, view, plotW, laneTop, laneH);
    return;
  }
  if (style === "flag") {
    drawFlagRibbon(ctx, trace, view, plotW, laneTop, laneH);
    return;
  }
  if (style === "heat") {
    drawHeatStrip(ctx, trace, view, plotW, laneTop, laneH);
    return;
  }
  drawWaveformTrace(ctx, trace, view, plotW, laneTop, laneH, style);
}

function drawWaveformTrace(
  ctx: CanvasRenderingContext2D,
  trace: Trace,
  view: { t0: number; t1: number },
  plotW: number,
  laneTop: number,
  laneH: number,
  style: "step" | "line" | "area",
): void {
  const points = trace.points;
  if (points.length === 0) return;
  const span = Math.max(1, view.t1 - view.t0);
  const yOf = (v: number) => yForTrace(v, trace, laneTop, laneH);
  const xOf = (t: number) => PAD.left + ((t - view.t0) / span) * plotW;
  const last = points[points.length - 1];
  const tailX = xOf(Math.max(last.t, Math.min(view.t1, trace.endT ?? view.t1)));

  const buildPath = () => {
    ctx.beginPath();
    ctx.moveTo(xOf(points[0].t), yOf(points[0].v));
    if (style === "line") {
      for (let i = 1; i < points.length; i += 1) {
        ctx.lineTo(xOf(points[i].t), yOf(points[i].v));
      }
      ctx.lineTo(tailX, yOf(last.v));
    } else {
      for (let i = 1; i < points.length; i += 1) {
        const prev = points[i - 1];
        const cur = points[i];
        ctx.lineTo(xOf(cur.t), yOf(prev.v));
        ctx.lineTo(xOf(cur.t), yOf(cur.v));
      }
      ctx.lineTo(tailX, yOf(last.v));
    }
  };

  if (style === "area" && !trace.dashed) {
    const baseVal = trace.min < 0 && trace.max > 0 ? 0 : trace.min;
    const baseY = clamp(yOf(baseVal), laneTop, laneTop + laneH);
    buildPath();
    ctx.lineTo(tailX, baseY);
    ctx.lineTo(xOf(points[0].t), baseY);
    ctx.closePath();
    ctx.save();
    ctx.fillStyle = trace.color;
    ctx.globalAlpha = 0.2;
    ctx.fill();
    ctx.restore();
  }

  buildPath();
  ctx.strokeStyle = trace.color;
  ctx.globalAlpha = trace.dashed ? 0.8 : 1;
  ctx.lineWidth = trace.dashed ? 1.15 : 1.5;
  ctx.lineJoin = "round";
  ctx.lineCap = "round";
  ctx.setLineDash(trace.dashed ? [5, 3] : []);
  ctx.stroke();
  ctx.setLineDash([]);
  ctx.globalAlpha = 1;
}

interface RibbonSegment {
  t0: number;
  t1: number;
  v: number;
}

function buildSegments(trace: Trace, view: { t0: number; t1: number }, roundKey: boolean): RibbonSegment[] {
  const pts = trace.points;
  if (pts.length === 0) return [];
  const endT = Math.max(pts[pts.length - 1].t, Math.min(view.t1, trace.endT ?? view.t1));
  const segs: RibbonSegment[] = [];
  for (let i = 0; i < pts.length; i += 1) {
    const t0 = Math.max(view.t0, pts[i].t);
    const t1 = Math.min(view.t1, i + 1 < pts.length ? pts[i + 1].t : endT);
    if (t1 <= t0) continue;
    const v = roundKey ? Math.round(pts[i].v) : pts[i].v;
    const prev = segs[segs.length - 1];
    if (prev && prev.v === v && Math.abs(prev.t1 - t0) <= 1) {
      prev.t1 = t1;
    } else {
      segs.push({ t0, t1, v });
    }
  }
  return segs;
}

function stateStyleFor(value: number, label: string | null): { bg: string; border: string; text: string } {
  const norm = label?.trim().toLowerCase() ?? "";
  if (
    value === 0 &&
    (!norm || norm === "off" || norm === "none" || norm === "idle" || norm === "inactive" || norm === "n" || norm === "neutral")
  ) {
    return STATE_NEUTRAL;
  }
  const idx = Math.abs(Math.round(value) * 3 + (value < 0 ? 5 : 0)) % STATE_PALETTE.length;
  return STATE_PALETTE[idx];
}

function drawStateRibbon(
  ctx: CanvasRenderingContext2D,
  trace: Trace,
  view: { t0: number; t1: number },
  plotW: number,
  laneTop: number,
  laneH: number,
): void {
  const segs = buildSegments(trace, view, true);
  if (segs.length === 0) return;
  const span = Math.max(1, view.t1 - view.t0);
  const xOf = (t: number) => PAD.left + ((t - view.t0) / span) * plotW;

  const bandH = Math.min(26, Math.max(12, laneH - 6));
  const bandY = laneTop + (laneH - bandH) / 2;
  const labelMap = new Map<number, string>((trace.labels ?? []).map((l) => [l.value, l.label]));

  ctx.save();
  ctx.globalAlpha = trace.dashed ? 0.65 : 1;
  ctx.font = `600 10px ${SANS}`;
  ctx.textBaseline = "middle";
  ctx.textAlign = "center";

  for (const seg of segs) {
    const x0 = xOf(seg.t0);
    const x1 = xOf(seg.t1);
    const w = x1 - x0;
    if (w <= 0.3) continue;
    const enumLabel = labelMap.get(seg.v) ?? null;
    const pal = stateStyleFor(seg.v, enumLabel);
    const drawW = Math.max(1, w - (w > 3 ? 1 : 0));

    ctx.fillStyle = pal.bg;
    if (drawW >= 6) {
      ctx.beginPath();
      roundRect(ctx, x0, bandY, drawW, bandH, 2);
      ctx.fill();
      ctx.fillStyle = pal.border;
      ctx.fillRect(x0 + 1, bandY, Math.max(1, drawW - 2), 2);
    } else {
      ctx.fillRect(x0, bandY, drawW, bandH);
    }

    if (drawW >= 18) {
      const text = enumLabel ?? String(seg.v);
      const maxChars = Math.floor((drawW - 6) / 6.2);
      if (maxChars >= 1) {
        const shown = text.length <= maxChars ? text : maxChars > 2 ? `${text.slice(0, maxChars - 1)}…` : text.slice(0, maxChars);
        ctx.fillStyle = pal.text;
        ctx.fillText(shown, x0 + drawW / 2, bandY + bandH / 2 + 0.5);
      }
    }
  }
  ctx.restore();
}

function drawFlagRibbon(
  ctx: CanvasRenderingContext2D,
  trace: Trace,
  view: { t0: number; t1: number },
  plotW: number,
  laneTop: number,
  laneH: number,
): void {
  const segs = buildSegments(trace, view, false);
  if (segs.length === 0) return;
  const span = Math.max(1, view.t1 - view.t0);
  const xOf = (t: number) => PAD.left + ((t - view.t0) / span) * plotW;

  const bandH = Math.min(20, Math.max(10, laneH - 8));
  const bandY = laneTop + (laneH - bandH) / 2;
  const lowY = bandY + bandH - 2;

  ctx.save();
  ctx.globalAlpha = trace.dashed ? 0.65 : 1;
  ctx.font = `600 9px ${MONO}`;
  ctx.textBaseline = "middle";
  ctx.textAlign = "center";

  // Subtle baseline track
  ctx.fillStyle = "rgba(180, 200, 210, 0.12)";
  ctx.fillRect(PAD.left, lowY, plotW, 2);

  const labelMap = new Map<number, string>((trace.labels ?? []).map((l) => [l.value, l.label]));
  const onLabel = labelMap.get(1) ?? "ON";

  for (const seg of segs) {
    if (seg.v <= 0.5) continue;
    const x0 = xOf(seg.t0);
    const x1 = xOf(seg.t1);
    const w = Math.max(1.5, x1 - x0);
    ctx.fillStyle = trace.color;
    ctx.beginPath();
    roundRect(ctx, x0, bandY, w, bandH, 2);
    ctx.fill();
    if (w >= 24) {
      ctx.fillStyle = "#0c1014";
      ctx.fillText(onLabel.slice(0, Math.floor((w - 6) / 6)), x0 + w / 2, bandY + bandH / 2 + 0.5);
    }
  }
  ctx.restore();
}

function heatColor(u: number): string {
  const c = clamp(u, 0, 1);
  // 5-stop perceptual ramp: deep blue -> cyan -> green -> amber -> red
  const stops = [
    [22, 56, 83],
    [42, 183, 202],
    [111, 214, 142],
    [230, 162, 60],
    [255, 90, 69],
  ];
  const scaled = c * (stops.length - 1);
  const idx = Math.min(stops.length - 2, Math.floor(scaled));
  const f = scaled - idx;
  const a = stops[idx];
  const b = stops[idx + 1];
  const r = Math.round(a[0] + (b[0] - a[0]) * f);
  const g = Math.round(a[1] + (b[1] - a[1]) * f);
  const bl = Math.round(a[2] + (b[2] - a[2]) * f);
  return `rgb(${r},${g},${bl})`;
}

function drawHeatStrip(
  ctx: CanvasRenderingContext2D,
  trace: Trace,
  view: { t0: number; t1: number },
  plotW: number,
  laneTop: number,
  laneH: number,
): void {
  const segs = buildSegments(trace, view, false);
  if (segs.length === 0) return;
  const span = Math.max(1, view.t1 - view.t0);
  const xOf = (t: number) => PAD.left + ((t - view.t0) / span) * plotW;
  const range = trace.max - trace.min || 1;
  const bandH = Math.min(20, Math.max(10, laneH - 8));
  const bandY = laneTop + (laneH - bandH) / 2;

  ctx.save();
  ctx.globalAlpha = trace.dashed ? 0.65 : 0.92;
  for (const seg of segs) {
    const x0 = xOf(seg.t0);
    const x1 = xOf(seg.t1);
    const w = Math.max(1, x1 - x0);
    const u = (seg.v - trace.min) / range;
    ctx.fillStyle = heatColor(u);
    ctx.fillRect(x0, bandY, w + 0.5, bandH);
  }
  ctx.restore();
}

export { LIME, ORANGE, SKY };
