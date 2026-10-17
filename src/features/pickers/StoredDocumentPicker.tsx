import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { formatMessage } from "../../i18n/messages";
import type { StoredDocumentListItemDto } from "../../ipc/bindings";
import { formatBytes, formatInstant, formatRelative } from "../../lib/format";
import { fetchDocumentPage, loadDocumentMetadata } from "../documents/api";
import {
  EntityPicker,
  type EntitySource,
  type PickerEntity,
  type PickerProps,
} from "./EntityPicker";
import { pickerKeys } from "./ProfilePicker";
import { rememberDocument, useRecentDocuments } from "./recentDocuments";

/** The pages read when the picker opens (50 documents each); a personal store is far below this. */
export const MAX_DOCUMENT_PAGES = 4;

const NONE: readonly StoredDocumentListItemDto[] = [];

type DocumentFacts = {
  id: string;
  displayName: string;
  byteSize: number | string;
  pageCount?: number | null;
  createdAt?: string;
  usedAt?: number;
};

function documentEntity(document: DocumentFacts): PickerEntity {
  const pages = document.pageCount ?? null;
  const parts = [
    formatBytes(document.byteSize),
    pages === null
      ? ""
      : formatMessage(pages === 1 ? "picker.document.pages.one" : "picker.document.pages.other", {
          count: pages,
        }),
    document.createdAt === undefined ? "" : formatInstant(document.createdAt),
    document.usedAt === undefined
      ? ""
      : formatMessage("picker.document.used", { when: formatRelative(document.usedAt) }),
  ];
  const description = parts.filter((part) => part !== "").join(", ");
  return {
    id: document.id,
    label: document.displayName,
    ...(description ? { description } : {}),
  };
}

function matches(document: { id: string; displayName: string }, query: string): boolean {
  const needle = query.toLowerCase();
  return (
    document.displayName.toLowerCase().includes(needle) ||
    document.id.toLowerCase().includes(needle)
  );
}

/** The stored documents, newest first: the first pages of the listing, followed by their cursors. */
async function fetchStoredDocuments(): Promise<StoredDocumentListItemDto[]> {
  const documents: StoredDocumentListItemDto[] = [];
  let cursor: string | null = null;
  for (let page = 0; page < MAX_DOCUMENT_PAGES; page += 1) {
    const result = await fetchDocumentPage(cursor);
    documents.push(...result.documents);
    if (result.nextCursor === null) {
      break;
    }
    cursor = result.nextCursor;
  }
  return documents;
}

/**
 * The options: the documents this window used lately come first, then the stored documents of the
 * listing. The typed text filters both by file name (or id) over the loaded pages; the picker has
 * already debounced it, so the filter costs no command call.
 */
function useDocumentOptions(query: string, enabled: boolean) {
  const recent = useRecentDocuments();
  const result = useQuery({
    queryKey: pickerKeys.documentList,
    enabled,
    staleTime: 0,
    placeholderData: keepPreviousData,
    queryFn: fetchStoredDocuments,
  });
  const listed = result.data ?? NONE;
  const recentIds = new Set(recent.map((document) => document.id));
  const shownRecent = recent.filter((document) => query === "" || matches(document, query));
  const shownListed = listed.filter(
    (document) => !recentIds.has(document.id) && (query === "" || matches(document, query)),
  );
  return {
    entities: [
      ...shownRecent.map((document) => documentEntity(document)),
      ...shownListed.map((document) => documentEntity(document)),
    ],
    fetching: enabled && result.isFetching,
    failed: enabled && result.isError,
    retry: () => void result.refetch(),
  };
}

// A pasted id is looked up by `documentLoadMetadata` to show its file name.
const documentSource: EntitySource = {
  get placeholder() {
    return formatMessage("picker.document.placeholder");
  },
  get emptyText() {
    return formatMessage("picker.document.empty");
  },
  get noMatchText() {
    return formatMessage("picker.document.noMatch");
  },
  get errorText() {
    return formatMessage("picker.document.error");
  },
  useOptions: useDocumentOptions,
  useResolved(id, enabled) {
    const result = useQuery({
      queryKey: pickerKeys.document(id),
      enabled,
      staleTime: 60_000,
      retry: false,
      queryFn: () => loadDocumentMetadata(id),
    });
    const data = result.data;
    useEffect(() => {
      if (data) {
        rememberDocument(data);
      }
    }, [data]);
    return data ? { id: data.id, label: data.displayName } : null;
  },
};

/**
 * Picks a stored PDF. It lists the stored documents (newest first, with size, pages and date),
 * puts the ones used in this window on top, filters by file name as the user types, and accepts a
 * pasted canonical UUID, whose file name it then looks up and shows.
 */
export function StoredDocumentPicker(props: PickerProps) {
  return <EntityPicker source={documentSource} {...props} />;
}
