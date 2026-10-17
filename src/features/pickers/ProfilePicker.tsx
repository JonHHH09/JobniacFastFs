import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { formatMessage } from "../../i18n/messages";
import type { ProfileIdentityDto } from "../../ipc/bindings";
import { fetchProfile, fetchProfilePage, searchProfiles } from "../profiles/api";
import {
  EntityPicker,
  type EntitySource,
  type PickerEntity,
  type PickerProps,
} from "./EntityPicker";

/** TanStack Query keys of the pickers. They are separate from the screens' own keys on purpose. */
export const pickerKeys = {
  profiles: (query: string) => ["pickers", "profiles", query] as const,
  profile: (id: string) => ["pickers", "profile", id] as const,
  jobs: (query: string) => ["pickers", "jobs", query] as const,
  job: (id: string) => ["pickers", "job", id] as const,
  document: (id: string) => ["pickers", "document", id] as const,
  documentList: ["pickers", "documents"] as const,
};

export function profileEntity(profile: ProfileIdentityDto): PickerEntity {
  const name = profile.fullName.trim();
  return name === ""
    ? { id: profile.id, label: profile.email }
    : { id: profile.id, label: name, ...(profile.email ? { description: profile.email } : {}) };
}

const NONE: readonly PickerEntity[] = [];

/** The first page of the list for "", else the search results. The command palette reuses it. */
export function useProfileOptions(query: string, enabled: boolean) {
  const result = useQuery({
    queryKey: pickerKeys.profiles(query),
    enabled,
    staleTime: 0,
    placeholderData: keepPreviousData,
    queryFn: async () =>
      query === ""
        ? (await fetchProfilePage(null)).profiles
        : (await searchProfiles(query)).profiles.map((match) => match.profile),
  });
  return {
    entities: result.data ? result.data.map(profileEntity) : NONE,
    fetching: enabled && result.isFetching,
    failed: enabled && result.isError,
    retry: () => void result.refetch(),
  };
}

const profileSource: EntitySource = {
  get placeholder() {
    return formatMessage("picker.profile.placeholder");
  },
  get emptyText() {
    return formatMessage("picker.profile.empty");
  },
  get errorText() {
    return formatMessage("picker.profile.error");
  },
  useOptions: useProfileOptions,
  useResolved(id, enabled) {
    const result = useQuery({
      queryKey: pickerKeys.profile(id),
      enabled,
      staleTime: 60_000,
      retry: false,
      queryFn: async () => profileEntity((await fetchProfile(id)).profile),
    });
    return result.data ?? null;
  },
};

/**
 * Picks a saved profile by name or email and writes its id to `onChange`. A pasted canonical UUID
 * is accepted as it is.
 */
export function ProfilePicker(props: PickerProps) {
  return <EntityPicker source={profileSource} {...props} />;
}
