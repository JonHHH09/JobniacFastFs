import type { PageImageRefDto, StoredDocumentDto } from "../../ipc/bindings";
import {
  type Args,
  fail,
  freshId,
  type HandlerMap,
  nowIso,
  pageOf,
  requestOf,
  seed,
  uuid,
} from "./util";

// Stored PDFs are metadata only; their "pages" are generated SVG placeholders.

export type MockDocument = StoredDocumentDto & {
  pageCount: number;
  text: string;
  /** The creation instant, which orders `documents_list` (newest first). */
  createdAt: string;
};

const PAGE_WIDTH = 595;
const PAGE_HEIGHT = 842;

const documents: Map<string, MockDocument> = new Map();

const CV_TEXT = [
  "Mara Teller",
  "Senior Backend Engineer",
  "mara.teller@example.com | Hamburg",
  "",
  "Summary",
  "Backend engineer with eight years of experience building data-heavy services in Rust and Go.",
  "",
  "Experience",
  "Lumenwork GmbH, Senior Backend Engineer, 2020-04 to present",
  "Lead developer of the ingestion pipeline (2 TB/day).",
  "Brightfield Systems, Software Engineer, 2016-02 to 2020-03",
  "",
  "Education",
  "Northgate University of Applied Sciences, M.Sc. Computer Science, 2015",
].join("\n");

const LETTER_TEXT = [
  "Sehr geehrte Damen und Herren,",
  "",
  "mit großem Interesse habe ich Ihre Stellenanzeige gelesen. In den vergangenen acht Jahren habe ich",
  "datenintensive Dienste entwickelt und betrieben und freue mich auf die Aufgabe bei Ihnen.",
  "",
  "Mit freundlichen Grüßen",
  "Mara Teller",
].join("\n");

function add(doc: MockDocument): MockDocument {
  documents.set(doc.id, doc);
  return doc;
}

for (const doc of seed<MockDocument>([
  {
    id: uuid(6, 1),
    displayName: "synthetic-cv.pdf",
    mediaType: "application/pdf",
    byteSize: 48213,
    pageCount: 2,
    text: CV_TEXT,
    createdAt: "2026-03-10T08:15:00Z",
  },
  {
    id: uuid(6, 2),
    displayName: "synthetic-cover-letter.pdf",
    mediaType: "application/pdf",
    byteSize: 21877,
    pageCount: 1,
    text: LETTER_TEXT,
    createdAt: "2026-03-12T16:40:00Z",
  },
  {
    id: uuid(6, 3),
    displayName: "synthetic-resume-de.pdf",
    mediaType: "application/pdf",
    byteSize: 1_482_560,
    pageCount: 3,
    text: CV_TEXT,
    createdAt: "2026-03-13T11:05:00Z",
  },
])) {
  add(doc);
}

/** One stored document, or undefined. */
export function storedDocument(id: string): MockDocument | undefined {
  return documents.get(id);
}

/** Stores a document made elsewhere (generation) and returns it. */
export function storeDocument(displayName: string, pageCount: number, text: string): MockDocument {
  return add({
    id: freshId(),
    displayName,
    mediaType: "application/pdf",
    byteSize: 18000 + pageCount * 9000,
    pageCount,
    text,
    createdAt: nowIso(),
  });
}

/** The document the id names, or the typed `not_found` error. */
export function requireDocument(id: unknown): MockDocument {
  return documents.get(String(id).trim()) ?? fail("not_found", "documents.error.notFound");
}

/** The metadata the Rust DTO carries (no page count or text). */
export function metadataOf(doc: MockDocument): StoredDocumentDto {
  return {
    id: doc.id,
    displayName: doc.displayName,
    mediaType: doc.mediaType,
    byteSize: doc.byteSize,
  };
}

function escapeXml(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/**
 * A white A4 page with a title and grey text lines, as a `data:` URL. The real app returns a
 * `jobniac-page` URL that Rust serves; Vite serves this mode without Tauri's CSP, so a data URL
 * loads in `<img>`.
 */
export function placeholderPageUrl(name: string, pageIndex: number, pageCount: number): string {
  const lines: string[] = [];
  let seedValue = (pageIndex + 1) * 7919 + name.length * 31;
  for (let row = 0; row < 38; row += 1) {
    seedValue = (seedValue * 1103515245 + 12345) % 2147483648;
    const width = row % 9 === 8 ? 0 : 200 + (seedValue % 300);
    if (width > 0) {
      lines.push(
        `<rect x="48" y="${150 + row * 17}" width="${width}" height="7" rx="3" fill="#c9ccd1"/>`,
      );
    }
  }
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${PAGE_WIDTH}" height="${PAGE_HEIGHT}" viewBox="0 0 ${PAGE_WIDTH} ${PAGE_HEIGHT}">
<rect width="100%" height="100%" fill="#ffffff"/>
<rect x="0.5" y="0.5" width="${PAGE_WIDTH - 1}" height="${PAGE_HEIGHT - 1}" fill="none" stroke="#dcdfe3"/>
<text x="48" y="78" font-family="sans-serif" font-size="22" font-weight="700" fill="#3a3f47">${escapeXml(name)}</text>
<text x="48" y="104" font-family="sans-serif" font-size="12" fill="#8a9099">Synthetic placeholder page ${pageIndex + 1} of ${pageCount}</text>
<rect x="48" y="120" width="499" height="1.5" fill="#dcdfe3"/>
${lines.join("\n")}
</svg>`;
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

/** The page reference of one page of a stored document. */
export function pageRefOf(doc: MockDocument, pageIndex: number): PageImageRefDto {
  if (!Number.isInteger(pageIndex) || pageIndex < 0 || pageIndex >= doc.pageCount) {
    fail("validation_error", "documents.error.validation");
  }
  return {
    documentId: doc.id,
    pageIndex,
    pageCount: doc.pageCount,
    width: PAGE_WIDTH,
    height: PAGE_HEIGHT,
    revision: "1",
    url: placeholderPageUrl(doc.displayName, pageIndex, doc.pageCount),
  };
}

const selections = new Map<string, string>();

/** Stored documents, newest first (a later insert wins a tie), as the Rust listing orders them. */
function newestFirst(): MockDocument[] {
  return [...documents.values()]
    .map((doc, index) => ({ doc, index }))
    .sort((a, b) => b.doc.createdAt.localeCompare(a.doc.createdAt) || b.index - a.index)
    .map(({ doc }) => doc);
}

export const documentHandlers: HandlerMap = {
  documents_capabilities_get: () => ({ documents: true }),
  documents_list: ({ cursor }: Args) => {
    const { items, nextCursor } = pageOf(newestFirst(), cursor, 50);
    return {
      documents: items.map((doc) => ({
        id: doc.id,
        displayName: doc.displayName,
        mediaType: doc.mediaType,
        byteSize: String(doc.byteSize),
        pageCount: doc.pageCount,
        createdAt: doc.createdAt,
      })),
      nextCursor,
    };
  },
  document_reset: () => null,
  document_choose_file: () => {
    const token = freshId();
    selections.set(token, "synthetic-resume.pdf");
    return { selectionToken: token, displayName: "synthetic-resume.pdf" };
  },
  document_import_selected: (args: Args) => {
    const name = selections.get(String(requestOf(args).selectionToken));
    if (!name) {
      fail("not_found", "documents.error.notFound");
    }
    return metadataOf(storeDocument(name, 2, CV_TEXT));
  },
  document_preview_selected: (args: Args) => {
    if (!selections.has(String(requestOf(args).selectionToken))) {
      fail("not_found", "documents.error.notFound");
    }
    return { text: CV_TEXT, truncated: false };
  },
  document_load_metadata: (args: Args) => metadataOf(requireDocument(requestOf(args).documentId)),
  document_preview_stored: (args: Args) => {
    const doc = requireDocument(requestOf(args).documentId);
    return { document: metadataOf(doc), extraction: { text: doc.text, truncated: false } };
  },
  document_render_page: (args: Args) => {
    const request = requestOf(args);
    return pageRefOf(requireDocument(request.documentId), Number(request.pageIndex));
  },
};
