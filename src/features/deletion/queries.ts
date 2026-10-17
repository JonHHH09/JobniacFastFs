import { useQuery } from "@tanstack/react-query";
import { fetchDeletionCapabilities } from "./api";

/** TanStack Query key of the deletion capability. It is cached for the session. */
export const deletionKeys = {
  capabilities: ["deletion", "capabilities"] as const,
};

/**
 * Whether the delete controls may be shown (`hasDeletionService`). The question is asked
 * only while `enabled` (a loaded entity), so a screen that shows no entity makes no call. Until
 * Rust answers `true`, the control stays hidden.
 */
export function useDeletionAvailable(enabled: boolean): boolean {
  const query = useQuery({
    queryKey: deletionKeys.capabilities,
    queryFn: fetchDeletionCapabilities,
    enabled,
    staleTime: Number.POSITIVE_INFINITY,
    retry: false,
  });
  return query.data?.deletion === true;
}
