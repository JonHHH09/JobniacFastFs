import {
  commands,
  type DeleteConfirmationDto,
  type DeletionCapabilitiesDto,
  type JobDeleteResultDto,
  type ProfileDeleteResultDto,
} from "../../ipc/bindings";
import { runCommand } from "../../ipc/commands";

/** Whether the profile and job delete use cases are composed. Rust answers; a failure reads as off. */
export function fetchDeletionCapabilities(): Promise<DeletionCapabilitiesDto> {
  return runCommand(() => commands.deletionCapabilitiesGet());
}

/** Step one of a profile delete: Rust issues the confirmation token. Nothing is deleted. */
export function requestProfileDelete(profileId: string): Promise<DeleteConfirmationDto> {
  return runCommand(() => commands.requestProfileDelete(profileId));
}

/** Step two of a profile delete: echoes the token back; Rust deletes only with a valid one. */
export function deleteProfile(
  profileId: string,
  confirmationToken: string,
): Promise<ProfileDeleteResultDto> {
  return runCommand(() => commands.deleteProfile(profileId, confirmationToken));
}

/** Step one of a job delete: Rust issues the confirmation token. Nothing is deleted. */
export function requestJobDelete(jobId: string): Promise<DeleteConfirmationDto> {
  return runCommand(() => commands.requestJobDelete(jobId));
}

/** Step two of a job delete: echoes the token back; Rust deletes only with a valid one. */
export function deleteJob(jobId: string, confirmationToken: string): Promise<JobDeleteResultDto> {
  return runCommand(() => commands.deleteJob(jobId, confirmationToken));
}
