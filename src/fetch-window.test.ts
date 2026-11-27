import { describe, expect, it } from "vitest";
import { ENGINE_MAX_POINTS, needsFetch, planFetch, viewPoints } from "./fetch-window";

const log = { t0: 0, t1: 60_000_000 };
const width = 1000;

describe("planFetch", () => {
  it("pads one span on each side at the view's point density", () => {
    const plan = planFetch({ t0: 20_000_000, t1: 24_000_000 }, log, width);
    expect(plan.t0).toBe(16_000_000);
    expect(plan.t1).toBe(28_000_000);
    expect(plan.maxPoints).toBe(3 * viewPoints(width));
  });

  it("clamps to the log and keeps the density", () => {
    const plan = planFetch({ t0: 0, t1: 4_000_000 }, log, width);
    expect(plan.t0).toBe(0);
    expect(plan.t1).toBe(8_000_000);
    expect(plan.maxPoints).toBe(2 * viewPoints(width));
  });

  it("stays within the engine's point cap", () => {
    const plan = planFetch({ t0: 20_000_000, t1: 24_000_000 }, log, 3000);
    expect(plan.maxPoints).toBeLessThanOrEqual(ENGINE_MAX_POINTS);
  });
});

describe("needsFetch", () => {
  const view = { t0: 20_000_000, t1: 24_000_000 };
  const fetched = planFetch(view, log, width);

  it("fetches when nothing is loaded", () => {
    expect(needsFetch(null, view, log, width)).toBe(true);
  });

  it("reuses the fetch for small pans inside it", () => {
    expect(needsFetch(fetched, view, log, width)).toBe(false);
    expect(needsFetch(fetched, { t0: 22_000_000, t1: 26_000_000 }, log, width)).toBe(false);
    expect(needsFetch(fetched, { t0: 18_000_000, t1: 22_000_000 }, log, width)).toBe(false);
  });

  it("fetches ahead when the view nears an edge", () => {
    expect(needsFetch(fetched, { t0: 23_500_000, t1: 27_500_000 }, log, width)).toBe(true);
    expect(needsFetch(fetched, { t0: 16_500_000, t1: 20_500_000 }, log, width)).toBe(true);
  });

  it("does not chase an edge that is the log's own", () => {
    const start = { t0: 0, t1: 4_000_000 };
    expect(needsFetch(planFetch(start, log, width), start, log, width)).toBe(false);
  });

  it("fetches when the view leaves the fetched range", () => {
    expect(needsFetch(fetched, { t0: 26_000_000, t1: 30_000_000 }, log, width)).toBe(true);
  });

  it("fetches after zooming in far enough to need finer points", () => {
    expect(needsFetch(fetched, { t0: 21_500_000, t1: 22_500_000 }, log, width)).toBe(true);
    expect(needsFetch(fetched, { t0: 20_500_000, t1: 23_500_000 }, log, width)).toBe(false);
  });

  it("fetches after zooming out past the fetched range", () => {
    expect(needsFetch(fetched, { t0: 10_000_000, t1: 30_000_000 }, log, width)).toBe(true);
  });
});
