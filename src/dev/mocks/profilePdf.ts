import type { IngestResultDto, SourceViewDto } from "../../ipc/bindings";
import { requireDocument } from "./documents";
import { profileGraph, profileHandlers } from "./profiles";
import { type Args, fail, freshId, type HandlerMap, nowIso, requestOf, uuid } from "./util";

// Ingesting a stored PDF creates a synthetic profile; replacing one needs a confirmation token.

type Staged = { documentId: string; profileId: string };
const staged = new Map<string, Staged>();
const sources = new Map<string, SourceViewDto>();

function link(profileId: string): SourceViewDto {
  const source: SourceViewDto = {
    sourceLinkId: freshId(),
    profileId,
    extractionId: freshId(),
    sourceType: "resumePdf",
    createdAt: nowIso(),
  };
  sources.set(profileId, source);
  return source;
}

function result(
  status: IngestResultDto["status"],
  documentId: string,
  profileId: string | null,
): IngestResultDto {
  const source = profileId ? sources.get(profileId) : undefined;
  return {
    status,
    documentId,
    profileId,
    extractionId: source?.extractionId ?? null,
    sourceLinkId: source?.sourceLinkId ?? null,
    candidate: null,
  };
}

export const profilePdfHandlers: HandlerMap = {
  profile_pdf_capabilities_get: () => ({ profilePdf: true }),
  profile_pdf_ingest: (args: Args) => {
    const request = requestOf(args);
    const doc = requireDocument(request.documentId);
    const existing =
      typeof request.existingProfileId === "string" ? request.existingProfileId : null;
    if (existing) {
      if (!profileGraph(existing)) {
        fail("not_found", "profile.error.notFound");
      }
      if (!request.overwriteExistingProfile) {
        return {
          kind: "result",
          result: {
            ...result("duplicateProfileCandidate", doc.id, null),
            candidate: {
              profileId: existing,
              matchedOn: ["email"],
              alreadyLinked: sources.has(existing),
            },
          },
        };
      }
      const token = freshId();
      staged.set(token, { documentId: doc.id, profileId: existing });
      return {
        kind: "confirmationRequired",
        token,
        existingProfileId: existing,
        expectedRevision: String(
          request.expectedRevision ?? profileGraph(existing)?.profile.revision,
        ),
        documentId: doc.id,
      };
    }
    const created = profileHandlers.create_profile({
      requestJson: JSON.stringify({
        fullName: "Imported Candidate",
        email: `imported.${uuid(7, Date.now() % 100000)}@example.com`,
        summary: doc.text.slice(0, 160),
        skills: [
          { skill: "Rust", normalizedSkill: "rust", category: "Languages", displayOrder: 0 },
        ],
      }),
    }) as { profile: { id: string } };
    link(created.profile.id);
    return { kind: "result", result: result("createdProfile", doc.id, created.profile.id) };
  },
  profile_pdf_confirm_replacement: (args: Args) => {
    const token = String(requestOf(args).token);
    const entry = staged.get(token);
    if (!entry) {
      return fail("not_found", "profilePdf.error.token");
    }
    staged.delete(token);
    const graph = profileGraph(entry.profileId);
    if (!graph) {
      return fail("not_found", "profile.error.notFound");
    }
    profileHandlers.update_profile({
      profileId: entry.profileId,
      expectedRevision: graph.profile.revision,
      requestJson: JSON.stringify({
        fullName: graph.profile.fullName,
        email: graph.profile.email,
        summary: requireDocument(entry.documentId).text.slice(0, 160),
      }),
    });
    link(entry.profileId);
    return result("updatedProfile", entry.documentId, entry.profileId);
  },
  profile_pdf_cancel_replacement: (args: Args) => {
    staged.delete(String(requestOf(args).token));
    return { restoredState: "ready" };
  },
  profile_pdf_lookup_source: (args: Args) => {
    const source = sources.get(String(requestOf(args).profileId));
    return source ? structuredClone(source) : fail("not_found", "profilePdf.error.noSource");
  },
};
