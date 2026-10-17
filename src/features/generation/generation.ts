import { formatMessage, type MessageId } from "../../i18n/messages";
import type {
  GenerationOutputDto,
  GenerationResultDto,
  GenerationResumeRequest,
  GenerationStandaloneRequest,
} from "../../ipc/bindings";
import { formatBytes, humanizeEnum } from "../../lib/format";

/** The PDF types of the screen, in the combo order. */
export const GENERATION_KINDS = [
  "standalone",
  "masterResume",
  "canadianResume",
  "canadianFrenchResume",
  "germanyResume",
  "germanCoverLetter",
] as const;

/** One PDF type of the screen. */
export type GenerationKind = (typeof GENERATION_KINDS)[number];

/** The UUID fields accept at most 36 characters. */
export const ID_MAX_LENGTH = 36;

/** The file name and the title accept at most 4096 characters. */
export const TEXT_MAX_LENGTH = 4096;

/** The raw text of every field. Only the fields of the selected kind are sent. */
export type GenerationFields = {
  fileName: string;
  title: string;
  body: string;
  profileId: string;
  jobId: string;
  resumeId: string;
  includeProjects: boolean;
};

/** The fields a kind shows. */
export type VisibleFields = {
  fileName: boolean;
  title: boolean;
  body: boolean;
  profileId: boolean;
  jobId: boolean;
  resumeId: boolean;
  includeProjects: boolean;
};

/** The fields that the selected kind shows. */
export function visibleFields(kind: GenerationKind): VisibleFields {
  const standalone = kind === "standalone";
  const german = kind === "germanyResume" || kind === "germanCoverLetter";
  return {
    fileName: standalone,
    title: standalone,
    body: standalone,
    profileId: !standalone,
    jobId: german,
    resumeId: kind === "germanCoverLetter",
    includeProjects: kind === "germanyResume",
  };
}

/** The Rust command and request of one generation, built from the visible fields only. */
export type GenerationCall =
  | { command: "standalone"; request: GenerationStandaloneRequest }
  | { command: "resume"; request: GenerationResumeRequest };

/**
 * Builds the request of the selected kind. Identifiers are trimmed here; Rust admits them.
 * A field the kind does not show is never part of the request.
 */
export function buildGenerationCall(
  kind: GenerationKind,
  fields: GenerationFields,
): GenerationCall {
  const profileId = fields.profileId.trim();
  const jobId = fields.jobId.trim();
  switch (kind) {
    case "standalone":
      return {
        command: "standalone",
        request: { fileName: fields.fileName, title: fields.title, body: fields.body },
      };
    case "masterResume":
      return { command: "resume", request: { kind, profileId } };
    case "canadianResume":
      return { command: "resume", request: { kind, profileId } };
    case "canadianFrenchResume":
      return { command: "resume", request: { kind, profileId } };
    case "germanyResume":
      return {
        command: "resume",
        request: { kind, profileId, jobId, includeProjects: fields.includeProjects },
      };
    case "germanCoverLetter":
      return {
        command: "resume",
        request: { kind, profileId, jobId, resumeId: fields.resumeId.trim() },
      };
  }
}

/** The copy id of a kind in the combo. */
export function kindMessageId(kind: GenerationKind): MessageId {
  return `generation.kind.${GENERATION_KINDS.indexOf(kind)}` as MessageId;
}

/** "1 page" or "3 pages". */
export function pagesText(count: number): string {
  return formatMessage(count === 1 ? "generation.pages.one" : "generation.pages.other", { count });
}

/** The label of one output: language, size and page count (`generation.output.label`). */
export function outputLabel(output: GenerationOutputDto): string {
  return formatMessage("generation.output.label", {
    prefix: humanizeEnum("labelPrefix", output.labelPrefix),
    size: formatBytes(output.byteSize),
    pages: pagesText(output.pageCount),
  });
}

/**
 * The summary of a result (`generation.summary.*`). A single PDF shows its size and page
 * count; the German pair shows the bilingual sentence.
 */
export function resultSummary(result: GenerationResultDto): string {
  if (result.variant === "germanTailored") {
    return formatMessage("generation.summary.germanTailored");
  }
  const first = result.outputs[0];
  if (first === undefined) {
    return "";
  }
  return formatMessage("generation.summary.single", {
    size: formatBytes(first.byteSize),
    pages: pagesText(first.pageCount),
  });
}

/**
 * The copy of a failed generation. Every failure, whatever its code, shows the generic copy.
 * `generation.busy` is only an accessible name, never a message.
 */
export function generationFailureMessage(): MessageId {
  return "generation.error.generic";
}
