import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { formatMessage } from "../../i18n/messages";
import type { JobSummaryDto } from "../../ipc/bindings";
import { fetchJob, fetchJobPage, searchJobs } from "../jobs/api";
import {
  EntityPicker,
  type EntitySource,
  type PickerEntity,
  type PickerProps,
} from "./EntityPicker";
import { pickerKeys } from "./ProfilePicker";

export function jobEntity(
  job: Pick<JobSummaryDto, "id" | "title" | "company" | "location">,
): PickerEntity {
  const where = [job.company, job.location].filter((part) => part.trim() !== "").join(", ");
  return {
    id: job.id,
    label: job.title.trim() === "" ? job.company : job.title,
    ...(where ? { description: where } : {}),
  };
}

const NONE: readonly PickerEntity[] = [];

/** The first page of the list for "", else the search results. The command palette reuses it. */
export function useJobOptions(query: string, enabled: boolean) {
  const result = useQuery({
    queryKey: pickerKeys.jobs(query),
    enabled,
    staleTime: 0,
    placeholderData: keepPreviousData,
    queryFn: async () =>
      query === ""
        ? (await fetchJobPage(null)).jobs
        : (await searchJobs(query)).jobs.map((match) => match.job),
  });
  return {
    entities: result.data ? result.data.map(jobEntity) : NONE,
    fetching: enabled && result.isFetching,
    failed: enabled && result.isError,
    retry: () => void result.refetch(),
  };
}

const jobSource: EntitySource = {
  get placeholder() {
    return formatMessage("picker.job.placeholder");
  },
  get emptyText() {
    return formatMessage("picker.job.empty");
  },
  get errorText() {
    return formatMessage("picker.job.error");
  },
  useOptions: useJobOptions,
  useResolved(id, enabled) {
    const result = useQuery({
      queryKey: pickerKeys.job(id),
      enabled,
      staleTime: 60_000,
      retry: false,
      queryFn: async () => jobEntity((await fetchJob(id)).job),
    });
    return result.data ?? null;
  },
};

/**
 * Picks a saved job by title or company and writes its id to `onChange`. A pasted canonical UUID is
 * accepted as it is.
 */
export function JobPicker(props: PickerProps) {
  return <EntityPicker source={jobSource} {...props} />;
}
