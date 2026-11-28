import { eachTick, fitCanvas } from "./canvas";
import { severityColor, type Severity } from "./family";
import { clamp, formatTimeForSpan, formatUs } from "./format";
import { MONO, PLAYHEAD, RULE, TEXT_MUTED, YELLOW } from "./theme";
import type { Point } from "./types";

export interface TimelineMark {
  t: number;
  kind: "event" | "mark";
  severity?: Severity;
}

export interface TimeRange {
  t0: number;
  t1: number;
}

/** Whether the view is zoomed in tightly enough for the timeline micro-map loupe to help. */
export function shouldShowLoupe(domain: TimeRange, view: TimeRange, timelineWidthPx = 800): boolean {
  const total = Math.max(1, domain.t1 - domain.t0);
  const viewSpan = Math.max(1, view.t1 - view.t0);
  const boxPx = (viewSpan / total) * Math.max(1, timelineWidthPx);
  return viewSpan < total * 0.25 && boxPx < 96;
}

/**
 * Compute the magnified micro-map domain centered around `anchorT`.
 * Keeps the active cyan viewport box roughly ~1/10 of the loupe width so dragging is always effortless.
 */
export function computeLoupeDomain(
  domain: TimeRange,
  view: TimeRange,
  anchorT: number,
): { range: TimeRange; factor: number } {
  const total = Math.max(1, domain.t1 - domain.t0);
  const viewSpan = Math.max(1, view.t1 - view.t0);
  const loupeSpan = clamp(viewSpan * 10, Math.min(total, Math.max(viewSpan * 4, 4_000)), total * 0.35);

  const viewMid = (view.t0 + view.t1) / 2;
  // If hovering near the active viewport box, gently center on it so the box is immediately grabbable.
  const center = Math.abs(anchorT - viewMid) <= loupeSpan * 0.4 ? viewMid : anchorT;
  let t0 = center - loupeSpan / 2;
  let t1 = t0 + loupeSpan;
  if (t0 < domain.t0) {
    t0 = domain.t0;
    t1 = Math.min(domain.t1, t0 + loupeSpan);
  }
  if (t1 > domain.t1) {
    t1 = domain.t1;
    t0 = Math.max(domain.t0, t1 - loupeSpan);
  }
  return {
    range: { t0, t1 },
    factor: Math.max(1, Math.round(total / Math.max(1, t1 - t0))),
  };
}

export function drawTimeline(
  canvas: HTMLCanvasElement,
  domain: TimeRange,
  view: TimeRange,
  playhead: number,
  marks: TimelineMark[],
  overview: Point[] | null,
  loupeHighlight?: TimeRange | null,
): void {
  const fitted = fitCanvas(canvas);
  if (!fitted) return;
  const { ctx, w, h } = fitted;
  const bg = ctx.createLinearGradient(0, 0, 0, h);
  bg.addColorStop(0, "#141b22");
  bg.addColorStop(1, "#0d1217");
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, w, h);

  const pad = 10;
  const tickH = 14;
  const laneH = 10;
  const trackTop = tickH;
  const trackH = Math.max(10, h - tickH - laneH - 6);
  const laneTop = trackTop + trackH + 3;
  const span = Math.max(1, domain.t1 - domain.t0);
  const xOf = (t: number) => pad + ((t - domain.t0) / span) * (w - pad * 2);

  ctx.fillStyle = "#0b1014";
  ctx.strokeStyle = RULE;
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.rect(pad + 0.5, trackTop + 0.5, w - pad * 2 - 1, trackH - 1);
  ctx.fill();
  ctx.stroke();

  if (overview && overview.length > 1) {
    // Compute global min/max so the micro-map keeps consistent vertical scale with the main overview
    let min = Infinity;
    let max = -Infinity;
    for (const point of overview) {
      if (point.v < min) min = point.v;
      if (point.v > max) max = point.v;
    }
    const range = max - min || 1;
    ctx.save();
    ctx.beginPath();
    ctx.rect(pad + 1, trackTop + 1, Math.max(1, w - pad * 2 - 2), Math.max(1, trackH - 2));
    ctx.clip();
    ctx.beginPath();
    ctx.strokeStyle = "rgba(242, 227, 148, 0.75)";
    ctx.lineWidth = 1;
    let started = false;
    for (let i = 0; i < overview.length; i += 1) {
      const point = overview[i];
      if (i + 1 < overview.length && overview[i + 1].t < domain.t0) continue;
      const x = xOf(point.t);
      const y = trackTop + trackH - ((point.v - min) / range) * (trackH - 6) - 3;
      if (!started) {
        ctx.moveTo(x, y);
        started = true;
      } else {
        ctx.lineTo(x, y);
      }
      if (point.t > domain.t1) break;
    }
    ctx.stroke();
    ctx.restore();
  }

  // Highlight the slice currently magnified in the micro-map loupe
  if (loupeHighlight && loupeHighlight.t1 > domain.t0 && loupeHighlight.t0 < domain.t1) {
    const lx0 = clamp(xOf(loupeHighlight.t0), pad, w - pad);
    const lx1 = clamp(xOf(loupeHighlight.t1), pad, w - pad);
    ctx.fillStyle = "rgba(230, 162, 60, 0.12)";
    ctx.fillRect(lx0, trackTop, Math.max(3, lx1 - lx0), trackH);
    ctx.strokeStyle = "rgba(230, 162, 60, 0.65)";
    ctx.setLineDash([2, 2]);
    ctx.strokeRect(lx0 + 0.5, trackTop + 0.5, Math.max(2, lx1 - lx0 - 1), trackH - 1);
    ctx.setLineDash([]);
  }

  if (view.t1 >= domain.t0 && view.t0 <= domain.t1) {
    const vx0 = clamp(xOf(view.t0), pad, w - pad);
    const vx1 = clamp(xOf(view.t1), pad, w - pad);
    const vw = Math.max(2, vx1 - vx0);
    ctx.fillStyle = "rgba(62, 198, 255, 0.16)";
    ctx.fillRect(vx0, trackTop, vw, trackH);
    ctx.strokeStyle = "rgba(62, 198, 255, 0.9)";
    ctx.strokeRect(vx0 + 0.5, trackTop + 0.5, Math.max(1, vw - 1), trackH - 1);
  }

  ctx.fillStyle = "#10161c";
  ctx.fillRect(pad, laneTop, w - pad * 2, laneH);
  ctx.strokeStyle = "#1e2830";
  ctx.strokeRect(pad + 0.5, laneTop + 0.5, w - pad * 2 - 1, laneH - 1);

  // Bucket events per pixel column, keeping the highest severity (`fault > warn > info`)
  // and painting faults last so critical markers are never painted over by dense info ticks.
  const maxPx = Math.max(1, Math.ceil(w));
  const colRank = new Uint8Array(maxPx + 1);
  const bookmarkXs: number[] = [];
  for (const mark of marks) {
    if (mark.t < domain.t0 || mark.t > domain.t1) continue;
    const x = xOf(mark.t);
    if (mark.kind === "mark") {
      bookmarkXs.push(x);
      continue;
    }
    const col = Math.max(0, Math.min(maxPx, Math.round(x)));
    const rank = mark.severity === "fault" ? 3 : mark.severity === "warn" ? 2 : 1;
    if (rank > colRank[col]) colRank[col] = rank;
  }

  const severities: readonly [number, Severity][] = [
    [1, "info"],
    [2, "warn"],
    [3, "fault"],
  ];
  for (const [rank, sev] of severities) {
    ctx.fillStyle = severityColor(sev);
    for (let col = 0; col <= maxPx; col += 1) {
      if (colRank[col] === rank) {
        ctx.fillRect(col - 1.5, laneTop + 1, 3, laneH - 2);
      }
    }
  }

  ctx.fillStyle = YELLOW;
  for (const x of bookmarkXs) {
    ctx.beginPath();
    ctx.moveTo(x, trackTop + 1);
    ctx.lineTo(x - 3.5, trackTop + 7);
    ctx.lineTo(x + 3.5, trackTop + 7);
    ctx.closePath();
    ctx.fill();
  }

  if (playhead >= domain.t0 && playhead <= domain.t1) {
    const px = xOf(playhead);
    ctx.strokeStyle = PLAYHEAD;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(px + 0.5, trackTop - 2);
    ctx.lineTo(px + 0.5, laneTop + laneH);
    ctx.stroke();
  }

  ctx.font = `10px ${MONO}`;
  ctx.textBaseline = "top";
  eachTick(domain.t0, domain.t1, (t) => {
    const x = xOf(t);
    if (x < pad || x > w - pad - 42) return;
    ctx.fillStyle = "#31404a";
    ctx.fillRect(x, tickH - 4, 1, 3);
    ctx.fillStyle = TEXT_MUTED;
    ctx.fillText(span < 100_000 ? formatTimeForSpan(t, span) : formatUs(t), x + 3, 1);
  });
}

export function timeAt(canvas: HTMLCanvasElement, clientX: number, domain: TimeRange): number {
  const rect = canvas.getBoundingClientRect();
  const pad = 10;
  const span = Math.max(1, domain.t1 - domain.t0);
  const u = (clientX - rect.left - pad) / Math.max(1, rect.width - pad * 2);
  const clamped = Math.min(1, Math.max(0, u));
  return domain.t0 + clamped * span;
}
