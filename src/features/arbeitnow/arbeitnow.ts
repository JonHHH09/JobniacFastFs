import { isMessageId, type MessageId } from "../../i18n/messages";
import type {
  ArbeitnowImportResultDto,
  ArbeitnowImportStatusDto,
  ArbeitnowPageDto,
  ArbeitnowRowDto,
  ArbeitnowScanRequestDto,
} from "../../ipc/bindings";

/** Longest search terms the field accepts (UI only; Rust caps the trimmed text). */
export const QUERY_MAX_LENGTH = 256;
/** Longest location the field accepts (UI only; Rust caps the trimmed text). */
export const LOCATION_MAX_LENGTH = 128;

/** The Rust copy id of a busy refusal. The controls are locked while busy, so it has no copy. */
export const BUSY_MESSAGE_ID = "arbeitnow.busy";

/** The operations of the screen: a fixed-board scan, the next page and one row import. */
export type ArbeitnowOperation = "scan" | "next" | "import";

/** The raw form. Tags and job types are sent as typed; Rust splits and trims them. */
export type ArbeitnowForm = {
  query: string;
  location: string;
  tags: string;
  jobTypes: string;
  remoteOnly: boolean;
  visaSponsorship: boolean;
};

export const EMPTY_FORM: ArbeitnowForm = {
  query: "",
  location: "",
  tags: "",
  jobTypes: "",
  remoteOnly: false,
  visaSponsorship: false,
};

/** The import status shown on a row: `none` until this session imports it. */
export type RowImportStatus = "none" | ArbeitnowImportStatusDto;

/** A result row with the status the screen tracks for it. */
export type ArbeitnowRowView = Omit<ArbeitnowRowDto, "importStatus"> & {
  importStatus: RowImportStatus;
};

/**
 * The screen state. `busy` is the one running operation (every control is locked meanwhile). `message`
 * is the status line once nothing runs: a failure copy, a result copy or nothing.
 */
export type ArbeitnowView = {
  busy: ArbeitnowOperation | null;
  rows: ArbeitnowRowView[];
  hasNextPage: boolean;
  notice: string;
  message: MessageId | null;
};

export const INITIAL_VIEW: ArbeitnowView = {
  busy: null,
  rows: [],
  hasNextPage: false,
  notice: "",
  message: null,
};

const PROGRESS: Record<ArbeitnowOperation, MessageId> = {
  scan: "arbeitnow.progress.scan",
  next: "arbeitnow.progress.next",
  import: "arbeitnow.progress.import",
};

const GENERIC_FAILURE: Record<ArbeitnowOperation, MessageId> = {
  scan: "arbeitnow.error.scan.generic",
  next: "arbeitnow.error.scan.generic",
  import: "arbeitnow.error.import.generic",
};

/** A blank text is sent as `null`; any other text is sent as typed. */
function blankToNull(text: string): string | null {
  return text.trim() === "" ? null : text;
}

/**
 * The scan request of the form. A blank query or location is sent as `null`; the text is otherwise
 * sent as typed, and Rust trims and caps it. The input `maxLength` bounds the text in the browser, so
 * no slicing happens here (a cut could split a surrogate pair). Flags are sent as booleans.
 */
export function buildScanRequest(form: ArbeitnowForm): ArbeitnowScanRequestDto {
  return {
    query: blankToNull(form.query),
    location: blankToNull(form.location),
    remoteOnly: form.remoteOnly,
    visaSponsorship: form.visaSponsorship,
    tags: form.tags,
    jobTypes: form.jobTypes,
  };
}

/**
 * The posting date (`YYYY-MM-DD`, UTC) as the user's locale shows a date ("Jan 2, 2026"). A value
 * that is not a date comes back as it is; null gives "".
 */
export function formatPostedDate(postedAt: string | null, locale?: string): string {
  if (postedAt === null || postedAt === "") {
    return "";
  }
  const date = new Date(`${postedAt}T00:00:00Z`);
  if (Number.isNaN(date.getTime())) {
    return postedAt;
  }
  return new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeZone: "UTC" }).format(date);
}

/**
 * The excerpt as it is shown: the service cuts it at a fixed length, so text that stops without a
 * sentence end is trimmed back to the last whole word and gets an ellipsis.
 */
export function tidyExcerpt(excerpt: string): string {
  const text = excerpt.trim();
  if (text === "" || /[.!?…)"”]$/.test(text)) {
    return text;
  }
  const cut = text.lastIndexOf(" ");
  return `${cut > 0 ? text.slice(0, cut) : text}…`;
}

/** The import button copy of a row: `Import`, `Imported` or `Already saved`. */
export function importButtonMessage(status: RowImportStatus): MessageId {
  if (status === "createdJob") {
    return "arbeitnow.button.imported";
  }
  if (status === "reusedExistingJob") {
    return "arbeitnow.button.alreadySaved";
  }
  return "arbeitnow.button.import";
}

/** A row import is allowed only while nothing runs and the row is not imported yet. */
export function canImportRow(view: ArbeitnowView, row: ArbeitnowRowView): boolean {
  return view.busy === null && row.importStatus === "none";
}

/** The copy of a failure: the Rust copy id when it names an Arbeitnow error, else the generic copy. */
export function failureMessage(operation: ArbeitnowOperation, messageId: string): MessageId {
  if (isMessageId(messageId) && messageId.startsWith("arbeitnow.error.")) {
    return messageId;
  }
  return GENERIC_FAILURE[operation];
}

/** The copy shown on the status line: the progress copy while busy, else the settled message. */
export function statusMessage(view: ArbeitnowView): MessageId | null {
  return view.busy === null ? view.message : PROGRESS[view.busy];
}

/** Locks the screen for one operation and clears the settled message. */
export function startOperation(view: ArbeitnowView, operation: ArbeitnowOperation): ArbeitnowView {
  return { ...view, busy: operation, message: null };
}

/**
 * A scan replaces the rows; a next page appends its rows. The status line says that nothing matched
 * only when the screen holds no row at all, and that older postings remain while a next page exists.
 */
export function finishPage(
  view: ArbeitnowView,
  operation: "scan" | "next",
  page: ArbeitnowPageDto,
): ArbeitnowView {
  const incoming: ArbeitnowRowView[] = page.rows.map((row) => ({ ...row, importStatus: "none" }));
  const rows = operation === "scan" ? incoming : [...view.rows, ...incoming];
  return {
    busy: null,
    rows,
    hasNextPage: page.hasNextPage,
    notice: page.consistencyNotice,
    message:
      rows.length > 0
        ? null
        : page.hasNextPage
          ? "arbeitnow.result.emptySoFar"
          : "arbeitnow.result.empty",
  };
}

/**
 * A failed operation keeps the rows, their import status and the next-page availability; only the
 * status line changes.
 */
export function failOperation(
  view: ArbeitnowView,
  operation: ArbeitnowOperation,
  messageId: string,
): ArbeitnowView {
  return {
    ...view,
    busy: null,
    message: messageId === BUSY_MESSAGE_ID ? null : failureMessage(operation, messageId),
  };
}

/** A successful import marks its row and reports whether the posting was new or already saved. */
export function finishImport(view: ArbeitnowView, result: ArbeitnowImportResultDto): ArbeitnowView {
  return {
    ...view,
    busy: null,
    rows: view.rows.map((row) =>
      row.rowId === result.rowId ? { ...row, importStatus: result.status } : row,
    ),
    message:
      result.status === "reusedExistingJob"
        ? "arbeitnow.result.reused"
        : "arbeitnow.result.created",
  };
}

/** The Rust copy id of a rejected command, or the empty string when the rejection carries none. */
export function rejectionMessageId(error: unknown): string {
  if (typeof error === "object" && error !== null && "messageId" in error) {
    const messageId = (error as { messageId: unknown }).messageId;
    return typeof messageId === "string" ? messageId : "";
  }
  return "";
}
