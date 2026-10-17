import {
  type InfiniteData,
  type QueryClient,
  useInfiniteQuery,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useState } from "react";
import type {
  CommandError,
  ProfileIdentityDto,
  ProfilePageDto,
  ProfileSearchDto,
} from "../../ipc/bindings";
import { isCommandError, unknownFailure } from "../../ipc/commands";
import { fetchProfilePage, searchProfiles } from "./api";

/**
 * TanStack Query keys of the profile screens. The list is refreshed only by Refresh; a save patches
 * the cached rows (`renameProfile`) and a delete removes them (`forgetProfile`).
 */
export const profileKeys = {
  all: ["profiles"] as const,
  list: ["profiles", "list"] as const,
  searchAll: ["profiles", "search"] as const,
  search: (query: string) => ["profiles", "search", query] as const,
  graph: (profileId: string) => ["profiles", "graph", profileId] as const,
  personal: (profileId: string) => ["profiles", "personal", profileId] as const,
};

/**
 * Shows a saved profile's new name and email in the cached list and search results. Only the cache
 * changes (the list query is manual, so invalidating it would refetch nothing).
 */
export function renameProfile(
  queryClient: QueryClient,
  profile: { id: string; fullName: string; email: string },
): void {
  const patch = (row: ProfileIdentityDto): ProfileIdentityDto =>
    row.id === profile.id ? { ...row, fullName: profile.fullName, email: profile.email } : row;
  queryClient.setQueryData<InfiniteData<ProfilePageDto, string | null>>(
    profileKeys.list,
    (data) =>
      data && {
        ...data,
        pages: data.pages.map((page) => ({ ...page, profiles: page.profiles.map(patch) })),
      },
  );
  queryClient.setQueriesData<ProfileSearchDto>(
    { queryKey: profileKeys.searchAll },
    (data) =>
      data && {
        ...data,
        profiles: data.profiles.map((match) => ({ ...match, profile: patch(match.profile) })),
      },
  );
}

/**
 * Drops a deleted profile from the cached list and search results, and forgets its graph and personal
 * details. Only the cache changes; the next refresh reads Rust again.
 */
export function forgetProfile(queryClient: QueryClient, profileId: string): void {
  queryClient.setQueryData<InfiniteData<ProfilePageDto, string | null>>(
    profileKeys.list,
    (data) =>
      data && {
        ...data,
        pages: data.pages.map((page) => ({
          ...page,
          profiles: page.profiles.filter((row) => row.id !== profileId),
        })),
      },
  );
  queryClient.setQueriesData<ProfileSearchDto>(
    { queryKey: profileKeys.searchAll },
    (data) =>
      data && {
        ...data,
        profiles: data.profiles.filter((row) => row.profile.id !== profileId),
      },
  );
  queryClient.removeQueries({ queryKey: profileKeys.graph(profileId), exact: true });
  queryClient.removeQueries({ queryKey: profileKeys.personal(profileId), exact: true });
}

/** The state of the profile list as the screen shows it. */
export type ProfileListStatus = "idle" | "loading" | "ready" | "empty" | "failed" | "unconfigured";

export type ProfileListView = {
  rows: ProfileIdentityDto[];
  hasNextPage: boolean;
  status: ProfileListStatus;
  refreshing: boolean;
  loadingNext: boolean;
  refresh: () => Promise<void>;
  loadNext: () => Promise<void>;
};

const listOptions = {
  initialPageParam: null as string | null,
  getNextPageParam: (page: ProfilePageDto) => page.nextCursor,
  retry: false,
} as const;

function failureStatus(failure: CommandError | null): ProfileListStatus | null {
  if (failure === null) {
    return null;
  }
  return failure.code === "not_configured" ? "unconfigured" : "failed";
}

/**
 * The profile list: page 1 on refresh (replacing the rows), later pages appended. A failed refresh
 * keeps the rows already shown and only changes the status. Nothing loads at startup.
 */
export function useProfileList(): ProfileListView {
  const queryClient = useQueryClient();
  const [failure, setFailure] = useState<CommandError | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [loadingNext, setLoadingNext] = useState(false);
  const query = useInfiniteQuery({
    ...listOptions,
    queryKey: profileKeys.list,
    queryFn: ({ pageParam }) => fetchProfilePage(pageParam),
    enabled: false,
  });

  const rows = query.data?.pages.flatMap((page) => page.profiles) ?? [];
  const pages = query.data?.pages ?? [];
  const lastPage = pages.length > 0 ? pages[pages.length - 1] : undefined;
  const hasNextPage = Boolean(lastPage?.nextCursor);
  const loaded = query.data !== undefined;

  async function refresh(): Promise<void> {
    setRefreshing(true);
    try {
      // fetchInfiniteQuery with one page replaces the cached pages only when page 1 succeeds.
      await queryClient.fetchInfiniteQuery({
        ...listOptions,
        queryKey: profileKeys.list,
        queryFn: ({ pageParam }) => fetchProfilePage(pageParam),
        pages: 1,
      });
      setFailure(null);
    } catch (error) {
      setFailure(isCommandError(error) ? error : unknownFailure);
    } finally {
      setRefreshing(false);
    }
  }

  async function loadNext(): Promise<void> {
    if (!hasNextPage) {
      return;
    }
    setLoadingNext(true);
    const result = await query.fetchNextPage();
    setFailure(
      result.error ? (isCommandError(result.error) ? result.error : unknownFailure) : null,
    );
    setLoadingNext(false);
  }

  // Precedence: unconfigured, loading, failed, empty or ready, idle.
  const failed = failureStatus(failure);
  const status: ProfileListStatus =
    failed === "unconfigured"
      ? "unconfigured"
      : refreshing || loadingNext
        ? "loading"
        : (failed ?? (loaded ? (rows.length === 0 ? "empty" : "ready") : "idle"));

  return { rows, hasNextPage, status, refreshing, loadingNext, refresh, loadNext };
}

/** The search results of a submitted query. Nothing runs until a query is submitted. */
export function useProfileSearch(submittedQuery: string) {
  return useQuery({
    queryKey: profileKeys.search(submittedQuery),
    queryFn: () => searchProfiles(submittedQuery),
    enabled: submittedQuery !== "",
    retry: false,
  });
}
