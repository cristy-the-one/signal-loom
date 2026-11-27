/**
 * Which stretch of the log the scope asks for. It fetches a window three
 * spans wide around the view and keeps drawing from it while the view stays
 * inside, so a pan or a playback step needs no round trip. A new fetch starts
 * once the view comes within a quarter span of an edge, or once zooming in has
 * left the fetched points too coarse for the new span.
 */

export interface Range {
  t0: number;
  t1: number;
}

/** What one fetch covered and at what point budget. */
export interface Fetched extends Range {
  maxPoints: number;
}

/** The engine's cap on points per series (`MAX_QUERY_POINTS`). */
export const ENGINE_MAX_POINTS = 8_000;
/** Spans of padding fetched on each side of the view. */
const PAD_SPANS = 1;
/** A new fetch starts when the view is closer than this many spans to an edge. */
const EDGE_SPANS = 0.25;
/** A fetch whose density fell below this share of the wanted density is refetched. */
const COARSE = 0.6;

/** Points wanted across the view itself: two per CSS pixel, 200 to 4000. */
export function viewPoints(widthPx: number): number {
  return Math.max(200, Math.min(4000, Math.round(widthPx * 2)));
}

/** Points per microsecond a fetch around `view` aims for, within the engine cap over the padded width. */
function wantedDensity(view: Range, widthPx: number): number {
  const span = Math.max(1, view.t1 - view.t0);
  return Math.min(viewPoints(widthPx), ENGINE_MAX_POINTS / (1 + 2 * PAD_SPANS)) / span;
}

/** The window and point budget to fetch for `view`, clamped to the log. */
export function planFetch(view: Range, bounds: Range, widthPx: number): Fetched {
  const span = Math.max(1, view.t1 - view.t0);
  const t0 = Math.max(bounds.t0, Math.floor(view.t0 - PAD_SPANS * span));
  const t1 = Math.min(bounds.t1, Math.ceil(view.t1 + PAD_SPANS * span));
  const maxPoints = Math.max(
    2,
    Math.min(ENGINE_MAX_POINTS, Math.round(wantedDensity(view, widthPx) * Math.max(1, t1 - t0))),
  );
  return { t0, t1, maxPoints };
}

/** Whether `fetched` cannot serve `view` well and a new fetch should start. */
export function needsFetch(fetched: Fetched | null, view: Range, bounds: Range, widthPx: number): boolean {
  if (!fetched) return true;
  if (view.t0 < fetched.t0 || view.t1 > fetched.t1) return true;
  const span = Math.max(1, view.t1 - view.t0);
  const edge = EDGE_SPANS * span;
  if (fetched.t0 > bounds.t0 && view.t0 - fetched.t0 < edge) return true;
  if (fetched.t1 < bounds.t1 && fetched.t1 - view.t1 < edge) return true;
  const density = fetched.maxPoints / Math.max(1, fetched.t1 - fetched.t0);
  return density < COARSE * wantedDensity(view, widthPx);
}
