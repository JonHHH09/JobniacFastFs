import { useQuery } from "@tanstack/react-query";
import type { MatchCapabilitiesDto } from "../../ipc/bindings";
import { fetchCapabilities } from "./api";

/** TanStack Query key of the matching and reviews capability flags, shared by both screens. */
export const matchCapabilityKey = ["match", "capabilities"] as const;

/**
 * The capability flags of the Matching and Reviews screens. Both ask the same question, so they
 * share one query: Rust is asked once per session, whichever screen opens first.
 */
export function useMatchCapabilities(): {
  data: MatchCapabilitiesDto | undefined;
  failed: boolean;
} {
  const query = useQuery({
    queryKey: matchCapabilityKey,
    queryFn: fetchCapabilities,
    staleTime: Number.POSITIVE_INFINITY,
    retry: false,
  });
  return { data: query.data, failed: query.isError };
}
