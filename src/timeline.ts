import { fitCanvas } from "./canvas";
import { severityColor, type Severity } from "./family";
import { formatUs, tickStep } from "./format";
import type { Point } from "./types";

export interface TimelineMark {
  t: number;
  kind: "event" | "mark";
  severity?: Severity;
}

export function drawTimeline(
  canvas: HTMLCanvasElement,
  domain: { t0: number; t1: number },
  view: { t0: number; t1: number },
  playhead: number,
  marks: TimelineMark[],
  overview: Point[] | null,
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
  ctx.strokeStyle = "#243039";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.rect(pad + 0.5, trackTop + 0.5, w - pad * 2 - 1, trackH - 1);
  ctx.fill();
  ctx.stroke();

  if (overview && overview.length > 1) {
    let min = Infinity;
    let max = -Infinity;
    for (const point of overview) {
      min = Math.min(min, point.v);
      max = Math.max(max, point.v);
    }
    const range = max - min || 1;
    ctx.beginPath();
    ctx.strokeStyle = "rgba(242, 227, 148, 0.75)";
    ctx.lineWidth = 1;
    overview.forEach((point, index) => {
      const x = xOf(point.t);
      const y = trackTop + trackH - ((point.v - min) / range) * (trackH - 6) - 3;
      if (index === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    });
    ctx.stroke();
  }

  const vx0 = xOf(view.t0);
  const vx1 = xOf(view.t1);
  ctx.fillStyle = "rgba(62, 198, 255, 0.12)";
  ctx.fillRect(vx0, trackTop, Math.max(2, vx1 - vx0), trackH);
  ctx.strokeStyle = "rgba(62, 198, 255, 0.85)";
  ctx.strokeRect(vx0 + 0.5, trackTop + 0.5, Math.max(1, vx1 - vx0 - 1), trackH - 1);

  ctx.fillStyle = "#10161c";
  ctx.fillRect(pad, laneTop, w - pad * 2, laneH);
  ctx.strokeStyle = "#1e2830";
  ctx.strokeRect(pad + 0.5, laneTop + 0.5, w - pad * 2 - 1, laneH - 1);

  for (const mark of marks) {
    const x = xOf(mark.t);
    if (mark.kind === "mark") {
      ctx.fillStyle = "#f2e394";
      ctx.beginPath();
      ctx.moveTo(x, trackTop + 1);
      ctx.lineTo(x - 3.5, trackTop + 7);
      ctx.lineTo(x + 3.5, trackTop + 7);
      ctx.closePath();
      ctx.fill();
      continue;
    }
    const color = severityColor(mark.severity ?? "info");
    ctx.fillStyle = color;
    ctx.fillRect(x - 2, laneTop + 1, 4, laneH - 2);
  }

  const px = xOf(playhead);
  ctx.strokeStyle = "#ff4d2e";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(px + 0.5, trackTop - 2);
  ctx.lineTo(px + 0.5, laneTop + laneH);
  ctx.stroke();

  ctx.font = "10px 'IBM Plex Mono', ui-monospace, monospace";
  ctx.textBaseline = "top";
  const step = tickStep(span);
  const first = Math.ceil(domain.t0 / step) * step;
  for (let t = first; t <= domain.t1; t += step) {
    const x = xOf(t);
    ctx.fillStyle = "#31404a";
    ctx.fillRect(x, tickH - 4, 1, 3);
    ctx.fillStyle = "#8b9aa6";
    ctx.fillText(formatUs(t), x + 3, 1);
  }
}

export function timeAt(canvas: HTMLCanvasElement, clientX: number, domain: { t0: number; t1: number }): number {
  const rect = canvas.getBoundingClientRect();
  const pad = 10;
  const span = Math.max(1, domain.t1 - domain.t0);
  const u = (clientX - rect.left - pad) / Math.max(1, rect.width - pad * 2);
  const clamped = Math.min(1, Math.max(0, u));
  return domain.t0 + clamped * span;
}
