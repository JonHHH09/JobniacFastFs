import type { SourceViewDto } from "../../ipc/bindings";
import { profileIds } from "./profiles";
import { type Args, fail, freshId, type HandlerMap, nowIso, requestOf } from "./util";

// The screen only reads the PDF source of a profile: the first synthetic profile has one, the rest do not.

const sources = new Map<string, SourceViewDto>();
const [first] = profileIds();
if (first !== undefined) {
  sources.set(first, {
    sourceLinkId: freshId(),
    profileId: first,
    extractionId: freshId(),
    sourceType: "resumePdf",
    createdAt: nowIso(),
  });
}

export const profilePdfHandlers: HandlerMap = {
  profile_pdf_lookup_source: (args: Args) => {
    const source = sources.get(String(requestOf(args).profileId));
    return source ? structuredClone(source) : fail("not_found", "profilePdf.error.noSource");
  },
};
