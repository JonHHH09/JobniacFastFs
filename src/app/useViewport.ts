import { useSyncExternalStore } from "react";

/** The navigation layout: full sidebar (960px and up), icon rail (600 to 959px), top bar below. */
export type Viewport = "wide" | "rail" | "compact";

const WIDE_QUERY = "(min-width: 960px)";
const RAIL_QUERY = "(min-width: 600px)";

function supported(): boolean {
  return typeof window !== "undefined" && typeof window.matchMedia === "function";
}

function subscribe(onChange: () => void): () => void {
  if (!supported()) {
    return () => undefined;
  }
  const lists = [window.matchMedia(WIDE_QUERY), window.matchMedia(RAIL_QUERY)];
  for (const list of lists) {
    list.addEventListener("change", onChange);
  }
  return () => {
    for (const list of lists) {
      list.removeEventListener("change", onChange);
    }
  };
}

function snapshot(): Viewport {
  if (!supported()) {
    return "wide";
  }
  if (window.matchMedia(WIDE_QUERY).matches) {
    return "wide";
  }
  return window.matchMedia(RAIL_QUERY).matches ? "rail" : "compact";
}

/** The current layout. Without `matchMedia` (a test DOM) it is the full sidebar. */
export function useViewport(): Viewport {
  return useSyncExternalStore(subscribe, snapshot, () => "wide");
}
