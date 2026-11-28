import { describe, expect, it } from "vitest";
import { computeLoupeDomain, shouldShowLoupe } from "./timeline";

describe("timeline micro-map loupe", () => {
  const domain = { t0: 0, t1: 600_000_000 }; // 10 min

  it("activates only when zoomed in tightly enough that the viewport box is narrow", () => {
    expect(shouldShowLoupe(domain, { t0: 0, t1: 600_000_000 }, 1000)).toBe(false);
    expect(shouldShowLoupe(domain, { t0: 100_000_000, t1: 300_000_000 }, 1000)).toBe(false);
    // 10s span on 10min log => ~16px box on 1000px timeline
    expect(shouldShowLoupe(domain, { t0: 240_000_000, t1: 250_000_000 }, 1000)).toBe(true);
    // 1s span on 10min log => ~1.6px box
    expect(shouldShowLoupe(domain, { t0: 240_000_000, t1: 241_000_000 }, 1000)).toBe(true);
  });

  it("magnifies the region around the viewport and clamps to log bounds", () => {
    const view = { t0: 240_000_000, t1: 241_000_000 }; // 1s span
    const { range, factor } = computeLoupeDomain(domain, view, 240_500_000);
    expect(range.t1 - range.t0).toBe(10_000_000); // 10x view span = 10s
    expect(factor).toBe(60); // 600s / 10s = 60x magnification
    expect(range.t0).toBeLessThanOrEqual(view.t0);
    expect(range.t1).toBeGreaterThanOrEqual(view.t1);

    // Near start edge clamps to domain.t0 without shrinking span
    const nearStart = computeLoupeDomain(domain, { t0: 0, t1: 1_000_000 }, 500_000);
    expect(nearStart.range.t0).toBe(0);
    expect(nearStart.range.t1).toBe(10_000_000);

    // Near end edge clamps to domain.t1
    const nearEnd = computeLoupeDomain(domain, { t0: 599_000_000, t1: 600_000_000 }, 599_500_000);
    expect(nearEnd.range.t1).toBe(600_000_000);
    expect(nearEnd.range.t0).toBe(590_000_000);
  });
});
