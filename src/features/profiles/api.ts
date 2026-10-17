import {
  commands,
  type PersonalDetailsEnvelopeDto,
  type ProfileGraphDto,
  type ProfilePageDto,
  type ProfileSearchDto,
  type ProjectPatchDto,
} from "../../ipc/bindings";
import { requestText, runCommand } from "../../ipc/commands";

/** One page of the profile list. `cursor` is null for the first page. */
export function fetchProfilePage(cursor: string | null): Promise<ProfilePageDto> {
  return runCommand(() => commands.listProfiles(cursor));
}

/** The complete graph of one profile. */
export function fetchProfile(profileId: string): Promise<ProfileGraphDto> {
  return runCommand(() => commands.getProfile(profileId));
}

/** Creates a profile from the write request (JSON sent as text). */
export function createProfile(request: Record<string, unknown>): Promise<ProfileGraphDto> {
  return runCommand(() => commands.createProfile(requestText(request)));
}

/** Replaces a profile under the loaded revision (a decimal string). */
export function updateProfile(
  profileId: string,
  expectedRevision: string,
  request: Record<string, unknown>,
): Promise<ProfileGraphDto> {
  return runCommand(() =>
    commands.updateProfile(profileId, expectedRevision, requestText(request)),
  );
}

/** Saves the changed fields of one project. */
export function updateProject(request: Record<string, unknown>): Promise<ProjectPatchDto> {
  return runCommand(() => commands.updateProject(requestText(request)));
}

/** Searches the profile postings for a trimmed, non-empty query. */
export function searchProfiles(query: string): Promise<ProfileSearchDto> {
  return runCommand(() => commands.searchProfiles(query));
}

/** The personal-details envelope of a profile. */
export function fetchPersonalDetails(profileId: string): Promise<PersonalDetailsEnvelopeDto> {
  return runCommand(() => commands.getPersonalDetails(profileId));
}

/** Saves the personal details under the loaded profile revision. */
export function updatePersonalDetails(
  profileId: string,
  expectedRevision: string,
  request: Record<string, unknown>,
): Promise<PersonalDetailsEnvelopeDto> {
  return runCommand(() =>
    commands.updatePersonalDetails(profileId, expectedRevision, requestText(request)),
  );
}
