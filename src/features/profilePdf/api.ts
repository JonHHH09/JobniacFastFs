import {
  type CancelReplacementDto,
  commands,
  type IngestResultDto,
  type IngestStepDto,
  type ProfilePdfCapabilitiesDto,
  type SourceViewDto,
} from "../../ipc/bindings";
import { requestText, runCommand } from "../../ipc/commands";

/** Whether the profile-from-PDF screen is usable (a database is configured). Rust answers; read-only, no slot. */
export function fetchProfilePdfCapabilities(): Promise<ProfilePdfCapabilitiesDto> {
  return runCommand(() => commands.profilePdfCapabilitiesGet());
}

/** The ingestion request as the screen collects it. Revision and budget are text; Rust parses them. */
export type IngestForm = {
  documentId: string;
  existingProfileId: string | null;
  overwrite: boolean;
  expectedRevision: string | null;
  maxCharacters: string | null;
};

/**
 * Starts an ingestion. A replacement comes back as a confirmation step with a token and does not
 * change anything until the token is confirmed.
 */
export function ingestProfileFromPdf(form: IngestForm): Promise<IngestStepDto> {
  return runCommand(() =>
    commands.profilePdfIngest(
      requestText({
        documentId: form.documentId,
        existingProfileId: form.existingProfileId,
        overwriteExistingProfile: form.overwrite,
        expectedRevision: form.expectedRevision,
        maxCharacters: form.maxCharacters,
      }),
    ),
  );
}

/** Confirms the staged replacement named by its token. */
export function confirmReplacement(token: string): Promise<IngestResultDto> {
  return runCommand(() => commands.profilePdfConfirmReplacement(requestText({ token })));
}

/** Drops the staged replacement named by its token. */
export function cancelReplacement(token: string): Promise<CancelReplacementDto> {
  return runCommand(() => commands.profilePdfCancelReplacement(requestText({ token })));
}

/** The PDF source of a profile. */
export function lookupProfilePdfSource(profileId: string): Promise<SourceViewDto> {
  return runCommand(() => commands.profilePdfLookupSource(requestText({ profileId })));
}
