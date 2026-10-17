import { describe, expect, it } from "vitest";
import {
  canGoToPage,
  canZoom,
  clampPageIndex,
  clampZoom,
  stepZoom,
  ZOOM_MAX,
  ZOOM_MIN,
} from "./viewer";

describe("viewer zoom bounds", () => {
  it("clamps into the offered range and falls back to the default for non-finite values", () => {
    expect(clampZoom(0)).toBe(ZOOM_MIN);
    expect(clampZoom(-3)).toBe(ZOOM_MIN);
    expect(clampZoom(7)).toBe(ZOOM_MAX);
    expect(clampZoom(0.5)).toBe(0.5);
    expect(clampZoom(Number.NaN)).toBe(1);
    expect(clampZoom(Number.POSITIVE_INFINITY)).toBe(1);
  });

  it("steps by a quarter, never leaves the range, and reports when a step is possible", () => {
    expect(stepZoom(1, -1)).toBe(0.75);
    expect(stepZoom(0.75, -1)).toBe(0.5);
    expect(stepZoom(0.25, -1)).toBe(0.25);
    expect(stepZoom(1, 1)).toBe(1);
    expect(stepZoom(0.5, 1)).toBe(0.75);
    expect(canZoom(1, 1)).toBe(false);
    expect(canZoom(1, -1)).toBe(true);
    expect(canZoom(0.25, -1)).toBe(false);
    expect(canZoom(0.25, 1)).toBe(true);
  });

  it("never offers a zoom above the Rust maximum of 1.0 (the viewer does not upscale)", () => {
    for (let zoom = 0.25; zoom <= 1; zoom += 0.25) {
      expect(stepZoom(zoom, 1)).toBeLessThanOrEqual(1);
      expect(stepZoom(zoom, -1)).toBeGreaterThan(0);
    }
  });
});

describe("viewer page bounds", () => {
  it("clamps a page index into the page count and truncates fractions", () => {
    expect(clampPageIndex(-1, 3)).toBe(0);
    expect(clampPageIndex(1, 3)).toBe(1);
    expect(clampPageIndex(9, 3)).toBe(2);
    expect(clampPageIndex(1.9, 3)).toBe(1);
    expect(clampPageIndex(Number.NaN, 3)).toBe(0);
    expect(clampPageIndex(0, 0)).toBe(0);
  });

  it("allows a page only inside the page count", () => {
    expect(canGoToPage(0, 3)).toBe(true);
    expect(canGoToPage(2, 3)).toBe(true);
    expect(canGoToPage(3, 3)).toBe(false);
    expect(canGoToPage(-1, 3)).toBe(false);
    expect(canGoToPage(0.5, 3)).toBe(false);
    expect(canGoToPage(0, 0)).toBe(false);
  });
});
