import { useQuery } from "@tanstack/react-query";
import { fetchJobLinkCapabilities } from "./api";

/**
 * TanStack Query keys of the jobs screens. The store reads each page, search and detail with
 * `fetchQuery` at `staleTime: 0`, so every read asks Rust and the cache only deduplicates the
 * in-flight request. No component observes these keys (the store renders its own snapshot). A
 * successful write invalidates the whole `jobs` tree so that a cached copy never survives it.
 */
export const jobKeys = {
  all: ["jobs"] as const,
  page: (cursor: string | null) => ["jobs", "page", cursor] as const,
  search: (query: string) => ["jobs", "search", query] as const,
  detail: (jobId: string) => ["jobs", "detail", jobId] as const,
};

/** TanStack Query key of the link capability. It is outside the `jobs` tree, so writes keep it. */
export const jobLinkKeys = {
  capabilities: ["jobLink", "capabilities"] as const,
};

/**
 * Whether the Add from link row may be shown (`hasLinkService`). Cached for the session;
 * until Rust answers `true`, the row stays hidden.
 */
export function useJobLinkAvailable(): boolean {
  const query = useQuery({
    queryKey: jobLinkKeys.capabilities,
    queryFn: fetchJobLinkCapabilities,
    staleTime: Number.POSITIVE_INFINITY,
    retry: false,
  });
  return query.data?.jobLink === true;
}
