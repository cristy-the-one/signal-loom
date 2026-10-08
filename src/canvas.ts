import { tickStep } from "./format";

export interface Fitted {
  ctx: CanvasRenderingContext2D;
  w: number;
  h: number;
}

export function fitCanvas(canvas: HTMLCanvasElement): Fitted | null {
  const rect = canvas.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const w = Math.max(1, rect.width);
  const h = Math.max(1, rect.height);
  const pw = Math.max(1, Math.round(w * dpr));
  const ph = Math.max(1, Math.round(h * dpr));
  if (canvas.width !== pw || canvas.height !== ph) {
    canvas.width = pw;
    canvas.height = ph;
  }
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return { ctx, w, h };
}

/** Calls `visit` with each round tick time from `t0` to `t1`, spaced for that span. */
export function eachTick(t0: number, t1: number, visit: (t: number) => void): void {
  const step = tickStep(Math.max(1, t1 - t0));
  for (let t = Math.ceil(t0 / step) * step; t <= t1; t += step) visit(t);
}

/** Adds a rounded rectangle to the current path. The caller begins and fills or strokes it. */
export function roundRect(
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
