import {
  keepPreviousData,
  type QueryClient,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { formatMessage } from "../../i18n/messages";
import type { MatchReportViewDto } from "../../ipc/bindings";
import { formatInstant } from "../../lib/format";
import { fetchJob } from "../jobs/api";
import { getReport, listReports } from "../matching/api";
import { fetchProfile } from "../profiles/api";
import {
  EntityPicker,
  type EntitySource,
  type PickerEntity,
  type PickerProps,
} from "./EntityPicker";
import { jobEntity } from "./JobPicker";
import { pickerKeys, profileEntity } from "./ProfilePicker";

const NONE: readonly PickerEntity[] = [];

/**
 * A saved report is named after its job; the profile, the date and the score are the description.
 * The names are read through the query cache under the keys of `useJobName` and `useProfileName`, so
 * a name the matching page already loaded costs no command call. A name that cannot load falls back
 * to the generic copy, never the id.
 */
async function reportEntity(client: QueryClient, view: MatchReportViewDto): Promise<PickerEntity> {
  const { id, jobId, profileId, createdAt, overallScore } = view.report;
  const [job, profile] = await Promise.all([
    client
      .fetchQuery({
        queryKey: pickerKeys.job(jobId),
        staleTime: 60_000,
        retry: false,
        queryFn: async () => jobEntity((await fetchJob(jobId)).job),
      })
      .catch(() => null),
    client
      .fetchQuery({
        queryKey: pickerKeys.profile(profileId),
        staleTime: 60_000,
        retry: false,
        queryFn: async () => profileEntity((await fetchProfile(profileId)).profile),
      })
      .catch(() => null),
  ]);
  const parts = [
    profile?.label,
    formatInstant(createdAt),
    formatMessage("picker.report.score", { overallScore }),
  ].filter((part): part is string => part !== undefined && part !== "");
  return {
    id,
    label: job?.label ?? formatMessage("matching.name.job"),
    description: parts.join(", "),
  };
}

/** The first page of stored reports, filtered by the typed text (there is no report search command). */
function useReportOptions(query: string, enabled: boolean) {
  const client = useQueryClient();
  const result = useQuery({
    queryKey: ["pickers", "reports"] as const,
    enabled,
    staleTime: 0,
    placeholderData: keepPreviousData,
    queryFn: async () => {
      const page = await listReports(null, null, null);
      return Promise.all(page.reports.map((view) => reportEntity(client, view)));
    },
  });
  const needle = query.toLowerCase();
  const entities = result.data
    ? result.data.filter(
        (entity) =>
          needle === "" ||
          `${entity.label} ${entity.description ?? ""}`.toLowerCase().includes(needle),
      )
    : NONE;
  return {
    entities,
    fetching: enabled && result.isFetching,
    failed: enabled && result.isError,
    retry: () => void result.refetch(),
  };
}

const reportSource: EntitySource = {
  get placeholder() {
    return formatMessage("picker.report.placeholder");
  },
  get emptyText() {
    return formatMessage("picker.report.empty");
  },
  get errorText() {
    return formatMessage("picker.report.error");
  },
  useOptions: useReportOptions,
  useResolved(id, enabled) {
    const client = useQueryClient();
    const result = useQuery({
      queryKey: [...pickerKeys.job(id), "report"] as const,
      enabled,
      staleTime: 60_000,
      retry: false,
      queryFn: async () => reportEntity(client, await getReport(id)),
    });
    return result.data ?? null;
  },
};

/**
 * Picks a saved comparison report (job title, profile, date and score) and writes its id to
 * `onChange`. A pasted canonical UUID is accepted as it is.
 */
export function ReportPicker(props: PickerProps) {
  return <EntityPicker source={reportSource} {...props} />;
}
