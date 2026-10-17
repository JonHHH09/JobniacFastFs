import { useQuery } from "@tanstack/react-query";
import { formatMessage } from "../../i18n/messages";
import { fetchJob } from "../jobs/api";
import { jobEntity } from "../pickers/JobPicker";
import { pickerKeys, profileEntity } from "../pickers/ProfilePicker";
import { fetchProfile } from "../profiles/api";

/** The display name of a saved profile, from the picker's cache. The fallback is generic, never the id. */
export function useProfileName(id: string): { label: string; description: string | null } {
  const result = useQuery({
    queryKey: pickerKeys.profile(id),
    staleTime: 60_000,
    retry: false,
    queryFn: async () => profileEntity((await fetchProfile(id)).profile),
  });
  return {
    label: result.data?.label ?? formatMessage("matching.name.profile"),
    description: result.data?.description ?? null,
  };
}

/** The display name of a saved job, from the picker's cache. The fallback is generic, never the id. */
export function useJobName(id: string): { label: string; description: string | null } {
  const result = useQuery({
    queryKey: pickerKeys.job(id),
    staleTime: 60_000,
    retry: false,
    queryFn: async () => jobEntity((await fetchJob(id)).job),
  });
  return {
    label: result.data?.label ?? formatMessage("matching.name.job"),
    description: result.data?.description ?? null,
  };
}
