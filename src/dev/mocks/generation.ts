import type { GenerationOutputDto, GenerationResultDto } from "../../ipc/bindings";
import { storeDocument } from "./documents";
import { jobDetail } from "./jobs";
import { profileGraph } from "./profiles";
import { type Args, fail, freshId, type HandlerMap } from "./util";

// Generated PDFs are stored as synthetic documents, so the document and artifact viewers can open them.

function output(
  name: string,
  labelPrefix: GenerationOutputDto["labelPrefix"],
  pages: number,
  text: string,
): GenerationOutputDto {
  const doc = storeDocument(name, pages, text);
  return { documentId: doc.id, labelPrefix, byteSize: doc.byteSize, pageCount: pages };
}

function single(name: string, pages: number, text: string): GenerationResultDto {
  return { variant: "single", resumeId: null, outputs: [output(name, "PDF", pages, text)] };
}

function requireProfile(id: unknown) {
  return profileGraph(String(id)) ?? fail("not_found", "generation.error.notFound");
}

function requireJob(id: unknown) {
  return jobDetail(String(id)) ?? fail("not_found", "generation.error.notFound");
}

export const generationHandlers: HandlerMap = {
  generation_capabilities_get: () => ({ generation: true }),
  generation_standalone: ({ request }: Args) => {
    const req = request as Record<string, unknown>;
    const body = String(req.body ?? "");
    const name = String(req.fileName ?? "").trim();
    if (body.trim() === "") {
      fail("validation_error", "generation.error.validation");
    }
    return single(
      name === "" ? "standalone.pdf" : name,
      Math.max(1, Math.ceil(body.length / 3000)),
      body,
    );
  },
  generation_resume: ({ request }: Args) => {
    const req = request as Record<string, unknown>;
    const profile = requireProfile(req.profileId);
    const who = profile.profile.fullName.replace(/\s+/g, "-").toLowerCase();
    const summary = profile.profile.summary ?? "";
    switch (req.kind) {
      case "masterResume":
        return single(`${who}-master-resume.pdf`, 2, summary);
      case "canadianResume":
        return single(`${who}-canadian-resume.pdf`, 2, summary);
      case "canadianFrenchResume":
        return single(`${who}-cv-canadien.pdf`, 2, summary);
      case "germanyResume": {
        const job = requireJob(req.jobId);
        const company = job.job.company.replace(/\s+/g, "-").toLowerCase();
        return {
          variant: "germanTailored",
          resumeId: freshId(),
          outputs: [
            output(`${who}-${company}-en.pdf`, "EN", 2, summary),
            output(`${who}-${company}-de.pdf`, "DE", 2, summary),
          ],
        } satisfies GenerationResultDto;
      }
      case "germanCoverLetter": {
        const job = requireJob(req.jobId);
        return single(`${who}-anschreiben.pdf`, 1, job.job.title);
      }
      default:
        return fail("validation_error", "generation.error.validation");
    }
  },
};
