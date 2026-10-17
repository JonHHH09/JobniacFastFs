import {
  commands,
  type MatchBatchPageDto,
  type MatchCapabilitiesDto,
  type MatchHistoryPageDto,
  type MatchReportViewDto,
} from "../../ipc/bindings";
import { requestText, runCommand } from "../../ipc/commands";

/**
 * Which comparison and review services are composed. Read-only, asked once when the page mounts; a
 * failed call means the screens cannot act and they show the unconfigured state.
 */
export function fetchCapabilities(): Promise<MatchCapabilitiesDto> {
  return runCommand(() => commands.matchCapabilitiesGet());
}

/** Compares one saved profile with one saved job. The IDs are trimmed by the caller; Rust admits them. */
export function analyzePair(profileId: string, jobId: string): Promise<MatchReportViewDto> {
  return runCommand(() => commands.matchAnalyzePair(requestText({ profileId, jobId })));
}

/** One bounded page of a profile's saved jobs. `cursor` is null for the first page. */
export function analyzePage(profileId: string, cursor: string | null): Promise<MatchBatchPageDto> {
  return runCommand(() => commands.matchAnalyzePage(requestText({ profileId, cursor })));
}

/** One page of stored reports. A null filter is absent; the cursor is null for the first page. */
export function listReports(
  profileId: string | null,
  jobId: string | null,
  cursor: string | null,
): Promise<MatchHistoryPageDto> {
  return runCommand(() => commands.matchListReports(requestText({ profileId, jobId, cursor })));
}

/** One stored report by its identifier, with its staleness. */
export function getReport(id: string): Promise<MatchReportViewDto> {
  return runCommand(() => commands.matchGetReport(requestText({ id })));
}
