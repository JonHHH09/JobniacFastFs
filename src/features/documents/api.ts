import {
  commands,
  type DocumentCapabilitiesDto,
  type PageImageRefDto,
  type SelectedPdfDto,
  type StoredDocumentDto,
  type StoredDocumentPageDto,
  type StoredPreviewDto,
  type TextPreviewDto,
} from "../../ipc/bindings";
import { requestText, runCommand } from "../../ipc/commands";

/** Whether the documents screen is usable (a database is configured). Rust answers; read-only, no slot. */
export function fetchDocumentsCapabilities(): Promise<DocumentCapabilitiesDto> {
  return runCommand(() => commands.documentsCapabilitiesGet());
}

/**
 * Clears the documents view on the Rust side. It is refused with `busy` while another document action
 * runs; the screen clears its own fields either way.
 */
export function resetDocumentView(): Promise<null> {
  return runCommand(() => commands.documentReset());
}

/**
 * Opens the native open dialog in Rust. Resolves to `null` when the user cancels, which changes
 * nothing. The path never reaches React: only the opaque selection token and the base name do.
 */
export function chooseDocument(): Promise<SelectedPdfDto | null> {
  return runCommand(() => commands.documentChooseFile());
}

/** Imports the PDF held under `selectionToken` into the store. The source file is kept. */
export function importSelectedDocument(selectionToken: string): Promise<StoredDocumentDto> {
  return runCommand(() => commands.documentImportSelected(requestText({ selectionToken })));
}

/** The untrusted text preview of the PDF held under `selectionToken`; nothing is imported. */
export function previewSelectedDocument(selectionToken: string): Promise<TextPreviewDto> {
  return runCommand(() => commands.documentPreviewSelected(requestText({ selectionToken })));
}

/** The safe metadata of a stored document. The ID is trimmed by the caller; Rust admits it. */
export function loadDocumentMetadata(documentId: string): Promise<StoredDocumentDto> {
  return runCommand(() => commands.documentLoadMetadata(requestText({ documentId })));
}

/**
 * One page of the stored PDFs, newest first. `cursor` is null for the first page. Read-only and
 * desktop only; no path, hash or content crosses.
 */
export function fetchDocumentPage(cursor: string | null): Promise<StoredDocumentPageDto> {
  return runCommand(() => commands.documentsList(cursor));
}

/** The untrusted text preview of a stored document, persisted only when `persist` is set. */
export function previewStoredDocument(
  documentId: string,
  persist: boolean,
): Promise<StoredPreviewDto> {
  return runCommand(() => commands.documentPreviewStored(requestText({ documentId, persist })));
}

/**
 * Renders one zero-based page of a stored document for the page-image viewer. The reply carries the
 * `jobniac-page` URL and the size of the raster; the PNG itself stays in Rust.
 */
export function renderDocumentPage(
  documentId: string,
  pageIndex: number,
  zoom: number,
): Promise<PageImageRefDto> {
  return runCommand(() =>
    commands.documentRenderPage(requestText({ documentId, pageIndex, zoom })),
  );
}
