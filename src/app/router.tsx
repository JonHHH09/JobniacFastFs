import {
  createRootRoute,
  createRoute,
  createRouter,
  type RouterHistory,
} from "@tanstack/react-router";
import { AppShell } from "./AppShell";
import { HomePage } from "./HomePage";
import { parseSection, type SectionId } from "./sections";

// Code-based routes (no file-routing plugin). The shell has one route, `/`, and the active section
// lives in its `?section=` search param. Sections are not path routes: with
// one route a section change never unmounts the others, so no unsaved draft can be dropped, and
// production serving needs no route fallback. An unknown value is the default section.
const rootRoute = createRootRoute({ component: AppShell });

const homeRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: HomePage,
  validateSearch: (search: Record<string, unknown>): { section: SectionId } => ({
    section: parseSection(search.section),
  }),
});

const routeTree = rootRoute.addChildren([homeRoute]);

/** A router over the given history (tests pass a memory history). */
export function createAppRouter(history?: RouterHistory) {
  return createRouter({ routeTree, history });
}

export const router = createAppRouter();

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
