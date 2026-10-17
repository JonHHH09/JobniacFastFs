import {
  type ArtifactCapabilitiesDto,
  type ArtifactExportDto,
  type ArtifactPreviewStateDto,
  commands,
  type PageImageRefDto,
} from "../../ipc/bindings";
import { requestText, runCommand } from "../../ipc/commands";

/** Whether the artifact viewer and the native export are admitted. Rust answers; a failed check reads as not admitted. */
export function fetchArtifactCapabilities(): Promise<ArtifactCapabilitiesDto> {
  return runCommand(() => commands.artifactCapabilitiesGet());
}

/** Loads a stored PDF and shows its first page. The ID is trimmed here; Rust admits it. */
export function previewArtifact(documentId: string): Promise<ArtifactPreviewStateDto> {
  return runCommand(() => commands.artifactPreviewDocument(requestText({ documentId })));
}

/**
 * Shows one zero-based page of the loaded PDF. The turn names the document of the viewer that makes it:
 * the Rust session is shared by every artifact viewer, and Rust refuses a turn for a document it no longer
 * holds. Rust also checks the index against the page count.
 */
export function showArtifactPage(documentId: string, pageIndex: number): Promise<PageImageRefDto> {
  return runCommand(() => commands.artifactShowPage(requestText({ documentId, pageIndex })));
}

/**
 * Publishes the stored PDF to a file the user picks in the native save dialog. Resolves to `null` when
 * the dialog is cancelled. The path never reaches React: only the file name comes back.
 */
export function exportArtifact(documentId: string): Promise<ArtifactExportDto | null> {
  return runCommand(() => commands.artifactExportDocument(requestText({ documentId })));
}
