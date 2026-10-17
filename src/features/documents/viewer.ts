/**
 * UI-only bounds of the page-image viewer. They are UX hints: Rust validates the page index against the
 * page count and the zoom against its own range (zero exclusive, at most 1.0) on every render request.
 */

/** The smallest zoom the viewer offers. */
export const ZOOM_MIN = 0.25;
/** The largest zoom the viewer offers; the viewer never upscales the raster. */
export const ZOOM_MAX = 1;
/** The step between two zoom levels. */
export const ZOOM_STEP = 0.25;
/** The zoom a freshly rendered page starts with. */
export const ZOOM_DEFAULT = 1;

/** Clamps a zoom into [ZOOM_MIN, ZOOM_MAX]; a non-finite value falls back to the default. */
export function clampZoom(zoom: number): number {
  if (!Number.isFinite(zoom)) {
    return ZOOM_DEFAULT;
  }
  return Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, zoom));
}

/** The zoom one step in (`direction` 1) or out (`direction` -1), clamped and rounded to the step. */
export function stepZoom(zoom: number, direction: 1 | -1): number {
  const stepped = Math.round((clampZoom(zoom) + direction * ZOOM_STEP) / ZOOM_STEP) * ZOOM_STEP;
  return clampZoom(Number(stepped.toFixed(2)));
}

/** Whether another zoom step in or out is possible from `zoom`. */
export function canZoom(zoom: number, direction: 1 | -1): boolean {
  return stepZoom(zoom, direction) !== clampZoom(zoom);
}

/**
 * Clamps a zero-based page index into [0, pageCount - 1]. A document without pages has no valid index,
 * so the result is 0 and the caller must not render.
 */
export function clampPageIndex(index: number, pageCount: number): number {
  if (!Number.isFinite(index) || pageCount < 1) {
    return 0;
  }
  return Math.min(pageCount - 1, Math.max(0, Math.trunc(index)));
}

/** Whether the viewer can move to `index` from the current page: inside the page count only. */
export function canGoToPage(index: number, pageCount: number): boolean {
  return Number.isInteger(index) && index >= 0 && index < pageCount;
}
