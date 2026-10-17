import { useQuery } from "@tanstack/react-query";
import { fetchArbeitnowCapabilities } from "./api";

/** TanStack Query key of the Arbeitnow capability (outside every other tree). */
export const arbeitnowKeys = {
  capabilities: ["arbeitnow", "capabilities"] as const,
};

/**
 * Whether the Arbeitnow screen may be shown (`hasArbeitnowService`). Cached for the
 * session; until Rust answers `true` the screen stays hidden.
 */
export function useArbeitnowAvailable(): boolean {
  const query = useQuery({
    queryKey: arbeitnowKeys.capabilities,
    queryFn: fetchArbeitnowCapabilities,
    staleTime: Number.POSITIVE_INFINITY,
    retry: false,
  });
  return query.data?.arbeitnow === true;
}
