import {
  commands,
  type GenerationCapabilitiesDto,
  type GenerationResultDto,
  type GenerationResumeRequest,
  type GenerationStandaloneRequest,
} from "../../ipc/bindings";
import { runCommand } from "../../ipc/commands";

/** Whether generation is admitted. Rust answers; a failed check reads as not admitted. */
export function fetchGenerationCapabilities(): Promise<GenerationCapabilitiesDto> {
  return runCommand(() => commands.generationCapabilitiesGet());
}

/** Generates a standalone PDF from free text. The result carries opaque document IDs only. */
export function generateStandalone(
  request: GenerationStandaloneRequest,
): Promise<GenerationResultDto> {
  return runCommand(() => commands.generationStandalone(request));
}

/** Generates a resume or a German cover letter of a stored profile. */
export function generateResume(request: GenerationResumeRequest): Promise<GenerationResultDto> {
  return runCommand(() => commands.generationResume(request));
}
