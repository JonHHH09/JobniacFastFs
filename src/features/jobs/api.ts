import {
  commands,
  type JobCaptureDto,
  type JobDetailDto,
  type JobLinkCapabilitiesDto,
  type JobPageDto,
  type JobSearchDto,
} from "../../ipc/bindings";
import { requestText, runCommand } from "../../ipc/commands";

/** Whether the link capture is composed. Rust answers; a failure reads as off (the row stays hidden). */
export function fetchJobLinkCapabilities(): Promise<JobLinkCapabilitiesDto> {
  return runCommand(() => commands.jobLinkCapabilitiesGet());
}

/**
 * Captures the job behind a public job link. The link is sent once,
 * as input; Rust fetches the page and never returns the link.
 */
export function addJobFromLink(url: string): Promise<JobCaptureDto> {
  return runCommand(() => commands.jobAddLink(url));
}

/** One page of the saved jobs. `cursor` is null for the first page. */
export function fetchJobPage(cursor: string | null): Promise<JobPageDto> {
  return runCommand(() => commands.listJobs(cursor));
}

/** The complete job, by canonical UUID text. */
export function fetchJob(jobId: string): Promise<JobDetailDto> {
  return runCommand(() => commands.getJob(jobId));
}

/** Saves pasted posting text. The text is sent untrimmed. */
export function addJobFromText(text: string): Promise<JobCaptureDto> {
  return runCommand(() => commands.addJobFromText(requestText({ text })));
}

/** Searches the saved jobs. The query is sent as entered; Rust trims and validates it. */
export function searchJobs(query: string): Promise<JobSearchDto> {
  return runCommand(() => commands.searchJobs(requestText({ query })));
}

/** Saves the changed fields of a job under the revision it was loaded at (a decimal string). */
export function updateJob(
  jobId: string,
  expectedRevision: string,
  request: Record<string, string>,
): Promise<JobDetailDto> {
  return runCommand(() => commands.updateJob(jobId, expectedRevision, requestText(request)));
}
