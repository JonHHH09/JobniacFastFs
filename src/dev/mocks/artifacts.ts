import type { ArtifactPreviewStateDto } from "../../ipc/bindings";
import { metadataOf, pageRefOf, requireDocument } from "./documents";
import { type Args, fail, type HandlerMap, requestOf } from "./util";

// The artifact viewer shows the document named by the last preview.
let loadedId: string | null = null;

export const artifactHandlers: HandlerMap = {
  artifact_capabilities_get: () => ({ artifacts: true, export: true }),
  artifact_preview_document: (args: Args): ArtifactPreviewStateDto => {
    const doc = requireDocument(requestOf(args).documentId);
    loadedId = doc.id;
    return {
      documentId: doc.id,
      pageCount: doc.pageCount,
      currentPage: 0,
      summary: {
        displayName: doc.displayName,
        pageCount: doc.pageCount,
        byteSize: String(doc.byteSize),
      },
      page: pageRefOf(doc, 0),
    };
  },
  artifact_show_page: (args: Args) => {
    if (loadedId === null) {
      fail("not_found", "artifacts.error.notLoaded");
    }
    return pageRefOf(requireDocument(loadedId), Number(requestOf(args).pageIndex));
  },
  artifact_export_document: (args: Args) => {
    const doc = requireDocument(requestOf(args).documentId);
    return { exportedName: metadataOf(doc).displayName };
  },
};
