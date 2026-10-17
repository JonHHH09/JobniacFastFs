import { useNavigate, useSearch } from "@tanstack/react-router";
import { useCallback } from "react";
import { parseSection, type SectionId } from "./sections";

/**
 * The active section and the way to change it. The section is the `?section=` search param of the
 * one route, so the browser history (back and forward) moves between sections.
 */
export function useActiveSection(): { active: SectionId; select: (id: SectionId) => void } {
  const search = useSearch({ strict: false }) as { section?: unknown };
  const navigate = useNavigate();
  const select = useCallback(
    (id: SectionId) => {
      void navigate({ to: "/", search: { section: id } });
    },
    [navigate],
  );
  return { active: parseSection(search.section), select };
}
