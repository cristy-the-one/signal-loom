import { fitCanvas } from "./canvas";
import { formatUs, tickStep } from "./format";
import type { Point } from "./types";

export interface TimelineMark {
  t: number;
  kind: "event" | "mark";
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
  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = "#10151a";
  ctx.fillRect(0, 0, w, h);

  const pad = 10;
  const trackTop = 18;
  const trackH = Math.max(8, h - trackTop - 8);
  const span = Math.max(1, domain.t1 - domain.t0);
  const xOf = (t: number) => pad + ((t - domain.t0) / span) * (w - pad * 2);

  ctx.fillStyle = "#1a2229";
  ctx.fillRect(pad, trackTop, w - pad * 2, trackH);

  if (overview && overview.length > 1) {
    let min = Infinity;
    let max = -Infinity;
    for (const point of overview) {
      min = Math.min(min, point.v);
      max = Math.max(max, point.v);
    }
    const range = max - min || 1;
    ctx.beginPath();
    ctx.strokeStyle = "rgba(62, 198, 255, 0.55)";
    ctx.lineWidth = 1;
    overview.forEach((point, index) => {
      const x = xOf(point.t);
      const y = trackTop + trackH - ((point.v - min) / range) * (trackH - 4) - 2;
      if (index === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    });
    ctx.stroke();
  }

  const vx0 = xOf(view.t0);
  const vx1 = xOf(view.t1);
  ctx.fillStyle = "rgba(230, 162, 60, 0.16)";
  ctx.fillRect(vx0, trackTop, Math.max(2, vx1 - vx0), trackH);
  ctx.strokeStyle = "rgba(230, 162, 60, 0.85)";
  ctx.lineWidth = 1;
  ctx.strokeRect(vx0 + 0.5, trackTop + 0.5, Math.max(1, vx1 - vx0 - 1), trackH - 1);

  for (const mark of marks) {
    const x = xOf(mark.t);
    ctx.fillStyle = mark.kind === "mark" ? "#e6a23c" : "#7eb6ff";
    ctx.beginPath();
    ctx.moveTo(x, trackTop);
    ctx.lineTo(x - 3.5, trackTop + 6);
    ctx.lineTo(x + 3.5, trackTop + 6);
    ctx.closePath();
    ctx.fill();
  }

  const px = xOf(playhead);
  ctx.strokeStyle = "#ff4d2e";
  ctx.beginPath();
  ctx.moveTo(px, trackTop - 2);
  ctx.lineTo(px, trackTop + trackH + 2);
  ctx.stroke();

  ctx.fillStyle = "#8b9aa6";
  ctx.font = "10px 'IBM Plex Mono', ui-monospace, monospace";
  ctx.textBaseline = "top";
  const step = tickStep(span);
  const first = Math.ceil(domain.t0 / step) * step;
  for (let t = first; t <= domain.t1; t += step) {
    const x = xOf(t);
    ctx.fillStyle = "#31404a";
    ctx.fillRect(x, 12, 1, 4);
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
