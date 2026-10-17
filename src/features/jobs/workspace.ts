import type { QueryClient } from "@tanstack/react-query";
import { isMessageId, type MessageId } from "../../i18n/messages";
import type {
  CaptureStatusDto,
  CommandError,
  JobDetailDto,
  JobFieldsDto,
  JobPageDto,
  JobSearchDto,
  JobSummaryDto,
} from "../../ipc/bindings";
import { isCommandError, unknownFailure } from "../../ipc/commands";
import { deleteJob, requestJobDelete } from "../deletion/api";
import {
  confirmationMessage,
  deleteFailureId,
  isBusyRefusal,
  isNotFound,
} from "../deletion/confirmation";
import { setShellDirty } from "../shell/closeGate";
import {
  addJobFromLink,
  addJobFromText,
  fetchJob,
  fetchJobPage,
  searchJobs,
  updateJob,
} from "./api";
import { jobKeys } from "./queries";

/** The fields the editor edits, in display order (skills are edited as text). */
export const EDIT_FIELDS = [
  "title",
  "company",
  "location",
  "description",
  "sourceLabel",
  "experienceRequirement",
  "employmentType",
  "seniority",
  "postedAt",
] as const;

/** One editable field name. */
export type EditField = (typeof EDIT_FIELDS)[number];

/** The editor values of one job, as text. */
export type JobDraft = Record<EditField, string>;

/** A catalogue message with its placeholder values. */
export type MessageRef = { id: MessageId; params?: Record<string, string | number> };

/** The operation that holds the job workspace. Only one runs at a time, as in the Rust guard. */
export type JobBusy =
  | "add"
  | "link"
  | "list"
  | "listMore"
  | "search"
  | "detail"
  | "update"
  | "delete";

/** A delete the user is asked to confirm: the Rust token, the job it names and the dialog text. */
export type JobDeleteTarget = { jobId: string; token: string; message: string };

/** A navigation request. While an operation runs, only the newest one is kept. */
export type JobNavigation =
  | { kind: "list" }
  | { kind: "search"; query: string }
  | { kind: "detail"; jobId: string };

/** Everything the job screens render. Replaced, never mutated, on each change. */
export type JobSnapshot = {
  /** True once Rust reports that the job service has no local database (`jobs.unconfigured`). */
  unconfigured: boolean;
  busy: JobBusy | null;
  pasteText: string;
  pasteMessage: MessageRef | null;
  /** The job link text the user typed. Shown in the input only; never copied into a message. */
  linkInput: string;
  linkMessage: MessageRef | null;
  saveStatus: CaptureStatusDto | null;
  searchInput: string;
  searchQuery: string;
  searchMessage: MessageRef | null;
  jobs: JobSummaryDto[];
  nextCursor: string | null;
  listMessage: MessageRef | null;
  /** True after the first page of the saved jobs failed to load, so the list is not known empty. */
  listFailed: boolean;
  message: MessageRef | null;
  current: JobDetailDto | null;
  draft: JobDraft;
  baseline: JobDraft;
  skillsDraft: string;
  baselineSkills: string;
  editorMessage: MessageRef | null;
  /** The delete waiting for confirmation, or null. Set only after Rust issued its token. */
  deleteTarget: JobDeleteTarget | null;
};

const EMPTY_DRAFT: JobDraft = {
  title: "",
  company: "",
  location: "",
  description: "",
  sourceLabel: "",
  experienceRequirement: "",
  employmentType: "",
  seniority: "",
  postedAt: "",
};

const INITIAL_SNAPSHOT: JobSnapshot = {
  unconfigured: false,
  busy: null,
  pasteText: "",
  pasteMessage: null,
  linkInput: "",
  linkMessage: null,
  saveStatus: null,
  searchInput: "",
  searchQuery: "",
  searchMessage: null,
  jobs: [],
  nextCursor: null,
  listMessage: null,
  listFailed: false,
  message: null,
  current: null,
  draft: EMPTY_DRAFT,
  baseline: EMPTY_DRAFT,
  skillsDraft: "",
  baselineSkills: "",
  editorMessage: null,
  deleteTarget: null,
};

/** The editable values of a job row, as the editor shows them. */
function draftOf(fields: JobFieldsDto): JobDraft {
  return {
    title: fields.title,
    company: fields.company,
    location: fields.location,
    description: fields.description,
    sourceLabel: fields.sourceLabel,
    experienceRequirement: fields.experienceRequirement,
    employmentType: fields.employmentType,
    seniority: fields.seniority,
    postedAt: fields.postedAt,
  };
}

/** The saved-jobs row of a job after a save (company and location are shown as text). */
function summaryOf(fields: JobFieldsDto): JobSummaryDto {
  return {
    id: fields.id,
    title: fields.title,
    company: fields.company,
    location: fields.location,
  };
}

/** A revision is usable for a save only when it is a non-negative decimal integer. */
function isRevision(value: string): boolean {
  return /^\d+$/.test(value);
}

/**
 * The skills the editor text names, as the editor reads them: split on line breaks,
 * each line stripped, empty lines dropped. Two texts that name the same skills are not a change.
 */
const LINE_BREAKS = ["\n", "\r", "\v", "\f", "\x1c", "\x1d", "\x1e", "\x85", "\u2028", "\u2029"];

function skillsOf(text: string): string[] {
  let lines = text.replace(/\r\n/g, "\n");
  for (const lineBreak of LINE_BREAKS) {
    lines = lines.split(lineBreak).join("\n");
  }
  return lines
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "");
}

function sameSkills(draft: string, baseline: string): boolean {
  const left = skillsOf(draft);
  const right = skillsOf(baseline);
  return left.length === right.length && left.every((skill, index) => skill === right[index]);
}

/** True when a failed command says the job service has no local database configured. */
function isUnconfigured(error: unknown): boolean {
  return isCommandError(error) && error.messageId === "jobs.unconfigured";
}

/**
 * The copy of a failed operation: the Rust message id when it is a catalogue id, otherwise the
 * operation's own copy. Raw error text never reaches the screen.
 */
function failureMessage(error: unknown, fallback: MessageId): MessageRef {
  const command: CommandError = isCommandError(error) ? error : unknownFailure;
  return { id: isMessageId(command.messageId) ? command.messageId : fallback };
}

/**
 * The job workspace store.
 *
 * It owns the UI state of the jobs screens and runs one operation at a time, as the Rust
 * single-flight guard does. A navigation request (refresh, open, search) that arrives
 * while an operation runs is kept as the newest pending request and runs when the operation ends.
 * A search that a newer request supersedes is ignored when it returns. Unsaved editor
 * edits block every navigation with a message and nothing else. Server data is read through the
 * query cache with the keys in `queries.ts`, and a successful write invalidates them.
 */
export class JobWorkspace {
  private snapshot: JobSnapshot = INITIAL_SNAPSHOT;
  private readonly listeners = new Set<() => void>();
  private readonly queryClient: QueryClient;
  private pending: JobNavigation | null = null;
  private searchGeneration = 0;
  private followUpList = false;
  // The dirty flag last reported to Rust; the close gate refuses while it is true.
  private reportedDirty = false;

  constructor(queryClient: QueryClient) {
    this.queryClient = queryClient;
  }

  /** Subscribes to snapshot changes (for `useSyncExternalStore`). */
  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  /** The current snapshot. Its identity changes only when the state changes. */
  readonly getSnapshot = (): JobSnapshot => this.snapshot;

  /** True when the editor holds values that differ from the loaded ones. */
  hasUnsavedChanges(): boolean {
    return this.isDirty();
  }

  /** True when Save job changes is enabled: a changed, loaded job with a usable revision, idle. */
  canSaveChanges(): boolean {
    const current = this.snapshot.current;
    return (
      current !== null &&
      this.snapshot.busy === null &&
      this.isDirty() &&
      isRevision(current.job.revision)
    );
  }

  /** Records the posting text the user typed. */
  setPasteText(text: string): void {
    this.patch({ pasteText: text });
  }

  /** Records the job link the user typed. */
  setLinkInput(text: string): void {
    this.patch({ linkInput: text });
  }

  /**
   * Captures the job behind a public link, then refreshes the saved jobs and selects the job (new or
   * reused). A blank link is refused before any request; the link is never echoed in a message.
   */
  saveLink(): void {
    if (this.snapshot.busy !== null || this.blockedByUnsavedEdits()) {
      return;
    }
    const link = this.snapshot.linkInput.trim();
    if (link === "") {
      this.patch({ saveStatus: null, linkMessage: { id: "job.validation.link" } });
      return;
    }
    this.patch({ saveStatus: null, linkMessage: { id: "job.progress.link" } });
    this.run(
      "link",
      async () => {
        const result = await addJobFromLink(link);
        this.patch({ linkInput: "", saveStatus: result.status, linkMessage: null });
        void this.queryClient.invalidateQueries({ queryKey: jobKeys.all });
        this.followUpList = true;
      },
      (error) => this.patch({ linkMessage: failureMessage(error, "job.error.link") }),
    );
  }

  /** Records the search text the user typed. */
  setSearchInput(text: string): void {
    this.patch({ searchInput: text });
  }

  /** Records one editor field the user changed. Ignored without a loaded job or while busy. */
  setField(field: EditField, value: string): void {
    if (this.snapshot.current === null || this.snapshot.busy !== null) {
      return;
    }
    this.patch({ draft: { ...this.snapshot.draft, [field]: value } });
  }

  /** Records the skills text the user typed (one skill per line). */
  setSkills(text: string): void {
    if (this.snapshot.current === null || this.snapshot.busy !== null) {
      return;
    }
    this.patch({ skillsDraft: text });
  }

  /** Captures the pasted posting. A blank paste is refused before any request. */
  savePaste(): void {
    if (this.snapshot.busy !== null || this.blockedByUnsavedEdits()) {
      return;
    }
    const text = this.snapshot.pasteText;
    if (text.trim() === "") {
      this.patch({ saveStatus: null, pasteMessage: { id: "job.validation.paste" } });
      return;
    }
    this.patch({ saveStatus: null, pasteMessage: null });
    this.run(
      "add",
      async () => {
        const result = await addJobFromText(text);
        this.patch({ pasteText: "", saveStatus: result.status });
        void this.queryClient.invalidateQueries({ queryKey: jobKeys.all });
        this.followUpList = true;
      },
      (error) => this.patch({ pasteMessage: failureMessage(error, "job.error.add") }),
    );
  }

  /** Reloads the first page of the saved jobs and leaves any search. */
  refresh(): void {
    this.navigate({ kind: "list" });
  }

  /** Appends the next page of the saved jobs, when there is one. */
  loadNextPage(): void {
    const cursor = this.snapshot.nextCursor;
    if (this.snapshot.busy !== null || cursor === null || this.blockedByUnsavedEdits()) {
      return;
    }
    this.patch({ listMessage: { id: "job.progress.listMore" } });
    this.run(
      "listMore",
      async () => {
        const page = await this.fetchPage(cursor);
        this.patch({
          jobs: [...this.snapshot.jobs, ...page.jobs],
          nextCursor: page.nextCursor,
          listMessage: null,
        });
      },
      (error) => this.patch({ listMessage: failureMessage(error, "job.error.list") }),
    );
  }

  /** Loads one job into the detail and the editor. */
  openJob(jobId: string): void {
    this.navigate({ kind: "detail", jobId });
  }

  /** Runs the search for the typed query. A blank query is refused before any request. */
  search(): void {
    if (this.blockedByUnsavedEdits()) {
      return;
    }
    const query = this.snapshot.searchInput;
    if (query.trim() === "") {
      if (this.snapshot.busy === null) {
        this.patch({ searchMessage: { id: "job.validation.search" } });
      }
      return;
    }
    this.navigate({ kind: "search", query });
  }

  /** Clears the search text and reloads the saved jobs. */
  showAllJobs(): void {
    this.patch({ searchInput: "" });
    this.refresh();
  }

  /** Saves the changed fields of the loaded job under its loaded revision. */
  saveChanges(): void {
    const current = this.snapshot.current;
    if (
      current === null ||
      this.snapshot.busy !== null ||
      !this.isDirty() ||
      !isRevision(current.job.revision)
    ) {
      return;
    }
    const request = this.changedFields();
    const postedAt = request.postedAt;
    if (postedAt !== undefined && postedAt.trim() === "") {
      this.patch({ editorMessage: { id: "job.validation.postedAt" } });
      return;
    }
    this.patch({ editorMessage: { id: "job.progress.update" } });
    this.run(
      "update",
      async () => {
        const saved = await updateJob(current.job.id, current.job.revision, request);
        this.applyDetail(saved);
        this.patch({
          jobs: this.snapshot.jobs.map((row) =>
            row.id === saved.job.id ? summaryOf(saved.job) : row,
          ),
          editorMessage: { id: "job.updated" },
        });
        void this.queryClient.invalidateQueries({ queryKey: jobKeys.all });
      },
      (error) => this.patch({ editorMessage: failureMessage(error, "job.error.update") }),
    );
  }

  /** Restores the loaded values. No confirmation. */
  discardChanges(): void {
    if (this.snapshot.busy !== null) {
      return;
    }
    this.patch({
      draft: this.snapshot.baseline,
      skillsDraft: this.snapshot.baselineSkills,
      editorMessage: { id: "job.discarded" },
    });
  }

  /** Discards the local edits and loads the latest version of the job. */
  reloadLatest(): void {
    const current = this.snapshot.current;
    if (current === null || this.snapshot.busy !== null) {
      return;
    }
    this.patch({
      draft: this.snapshot.baseline,
      skillsDraft: this.snapshot.baselineSkills,
      editorMessage: null,
    });
    this.loadJob(current.job.id, true);
  }

  /** True when Delete job is enabled: a job is loaded, the workspace is idle and no delete waits. */
  canDeleteJob(): boolean {
    return (
      this.snapshot.current !== null &&
      this.snapshot.busy === null &&
      this.snapshot.deleteTarget === null
    );
  }

  /**
   * Opens the delete confirmation for the loaded job. Rust issues the token first and nothing is
   * deleted. Unsaved edits do not block the delete; the dialog text says they will be discarded.
   */
  requestDelete(): void {
    const current = this.snapshot.current;
    if (current === null || !this.canDeleteJob()) {
      return;
    }
    const jobId = current.job.id;
    const message = confirmationMessage("job", current.job.title, this.isDirty());
    void requestJobDelete(jobId).then(
      (confirmation) => {
        // The token is dropped when the loaded job changed while Rust issued it.
        if (this.snapshot.current?.job.id === jobId && this.snapshot.busy === null) {
          this.patch({
            deleteTarget: { jobId, token: confirmation.confirmationToken, message },
          });
        }
      },
      (error: unknown) => this.reportDeleteFailure(error),
    );
  }

  /** Closes the confirmation without deleting anything. */
  cancelDelete(): void {
    if (this.snapshot.deleteTarget !== null) {
      this.patch({ deleteTarget: null });
    }
  }

  /** Deletes the confirmed job with the token Rust issued for it. A missing job leaves the list. */
  confirmDelete(): void {
    const target = this.snapshot.deleteTarget;
    if (target === null || this.snapshot.busy !== null) {
      return;
    }
    const previous = this.snapshot.message;
    this.patch({ deleteTarget: null, message: { id: "job.progress.delete" } });
    this.run(
      "delete",
      async () => {
        await deleteJob(target.jobId, target.token);
        this.forgetJob(target.jobId);
        this.patch({ message: { id: "job.deleted" } });
        void this.queryClient.invalidateQueries({ queryKey: jobKeys.all });
      },
      (error) => {
        if (isBusyRefusal(error)) {
          // Rust kept the token: the same confirmation opens again and the previous message returns.
          this.patch({ deleteTarget: target, message: previous });
          return;
        }
        if (isNotFound(error)) {
          // The job is already gone: it leaves the list and the editor, and nothing is reported as deleted.
          this.forgetJob(target.jobId);
          this.patch({ message: { id: "job.deleted.missing" } });
          return;
        }
        this.reportDeleteFailure(error);
      },
    );
  }

  /** Applies a navigation now, or keeps it as the newest pending request while work runs. */
  private navigate(navigation: JobNavigation): void {
    if (this.blockedByUnsavedEdits()) {
      return;
    }
    if (this.snapshot.busy !== null) {
      this.pending = navigation;
      if (this.snapshot.busy === "search") {
        // The running search is superseded: its result is ignored when it returns.
        this.searchGeneration += 1;
        this.patch({ searchMessage: null });
      }
      return;
    }
    switch (navigation.kind) {
      case "list":
        this.loadFirstPage();
        return;
      case "search":
        this.runSearch(navigation.query);
        return;
      case "detail":
        this.loadJob(navigation.jobId, false);
        return;
    }
  }

  private loadFirstPage(): void {
    this.patch({
      searchQuery: "",
      searchMessage: null,
      nextCursor: null,
      listMessage: { id: "job.progress.list" },
      listFailed: false,
    });
    this.run(
      "list",
      async () => {
        const page = await this.fetchPage(null);
        this.patch({
          jobs: page.jobs,
          nextCursor: page.nextCursor,
          listMessage: page.jobs.length === 0 ? { id: "job.list.empty" } : null,
        });
      },
      (error) =>
        this.patch({ listMessage: failureMessage(error, "job.error.list"), listFailed: true }),
    );
  }

  private runSearch(query: string): void {
    const generation = this.searchGeneration;
    this.patch({ searchMessage: { id: "job.progress.search" } });
    this.run(
      "search",
      async () => {
        const response = await this.fetchSearch(query);
        if (generation === this.searchGeneration) {
          this.applySearch(response);
        }
      },
      (error) => {
        if (generation === this.searchGeneration) {
          this.patch({ searchMessage: failureMessage(error, "job.error.search") });
        }
      },
    );
  }

  private loadJob(jobId: string, reload: boolean): void {
    this.patch({ message: { id: reload ? "job.progress.reload" : "job.progress.detail" } });
    this.run(
      "detail",
      async () => {
        const job = await this.fetchDetail(jobId);
        this.applyDetail(job);
        this.patch({ message: null });
      },
      (error) => this.patch({ message: failureMessage(error, "job.error.detail") }),
    );
  }

  /**
   * Starts one operation and settles it: the busy flag is cleared, then the newest pending
   * navigation runs, or the saved-jobs refresh that follows a successful capture.
   */
  private run(busy: JobBusy, task: () => Promise<void>, onFailure: (error: unknown) => void): void {
    this.patch({ busy });
    task()
      .catch((error: unknown) => {
        // The page status names the missing service; the controls are disabled from here on.
        if (isUnconfigured(error)) {
          this.patch({ unconfigured: true, message: { id: "jobs.unconfigured" } });
        }
        onFailure(error);
      })
      .finally(() => this.settle());
  }

  /**
   * Clears the busy flag, then runs the newest pending navigation. Otherwise a successful capture
   * refreshes the saved jobs (list only; the captured job is not selected).
   */
  private settle(): void {
    this.patch({ busy: null });
    const next = this.pending;
    const follow = this.followUpList;
    this.pending = null;
    this.followUpList = false;
    if (next !== null) {
      this.navigate(next);
    } else if (follow) {
      this.navigate({ kind: "list" });
    }
  }

  private applySearch(response: JobSearchDto): void {
    const count = response.jobs.length;
    let searchMessage: MessageRef;
    if (count === 0) {
      searchMessage = { id: "job.search.empty" };
    } else if (response.hasMore) {
      searchMessage = { id: "job.search.summary.more", params: { n: count } };
    } else {
      searchMessage = {
        id: "job.search.summary",
        params: { n: count, m: response.matchedCount },
      };
    }
    this.patch({
      searchQuery: response.query,
      jobs: response.jobs.map((match) => match.job),
      nextCursor: null,
      listMessage: null,
      listFailed: false,
      current: null,
      draft: EMPTY_DRAFT,
      baseline: EMPTY_DRAFT,
      skillsDraft: "",
      baselineSkills: "",
      editorMessage: null,
      searchMessage,
    });
  }

  private applyDetail(job: JobDetailDto): void {
    const draft = draftOf(job.job);
    const skills = job.skills.join("\n");
    this.patch({
      current: job,
      draft,
      baseline: draft,
      skillsDraft: skills,
      baselineSkills: skills,
      editorMessage: null,
    });
  }

  /** The fields that differ from the loaded values, as the request the Rust command reads. */
  private changedFields(): Record<string, string> {
    const { draft, baseline, skillsDraft, baselineSkills } = this.snapshot;
    const request: Record<string, string> = {};
    for (const field of EDIT_FIELDS) {
      if (draft[field] !== baseline[field]) {
        request[field] = draft[field];
      }
    }
    if (!sameSkills(skillsDraft, baselineSkills)) {
      request.skillsText = skillsDraft;
    }
    return request;
  }

  private isDirty(): boolean {
    const { draft, baseline, skillsDraft, baselineSkills } = this.snapshot;
    return (
      !sameSkills(skillsDraft, baselineSkills) ||
      EDIT_FIELDS.some((field) => draft[field] !== baseline[field])
    );
  }

  /** True, with the unsaved-edits message, when the editor holds changes that must be kept. */
  private blockedByUnsavedEdits(): boolean {
    if (!this.isDirty()) {
      return false;
    }
    this.patch({ message: { id: "job.validation.unsaved" } });
    return true;
  }

  /** Removes a deleted job from the list and, when it is the loaded one, from the editor. */
  private forgetJob(jobId: string): void {
    const loaded = this.snapshot.current?.job.id === jobId;
    this.patch({
      jobs: this.snapshot.jobs.filter((row) => row.id !== jobId),
      current: loaded ? null : this.snapshot.current,
      draft: loaded ? EMPTY_DRAFT : this.snapshot.draft,
      baseline: loaded ? EMPTY_DRAFT : this.snapshot.baseline,
      skillsDraft: loaded ? "" : this.snapshot.skillsDraft,
      baselineSkills: loaded ? "" : this.snapshot.baselineSkills,
      editorMessage: null,
    });
  }

  /** Shows the fixed delete failure copy; a refused call (busy) shows nothing. */
  private reportDeleteFailure(error: unknown): void {
    const failure = deleteFailureId("job", error);
    if (failure !== null) {
      this.patch({ message: { id: failure } });
    }
  }

  private fetchPage(cursor: string | null): Promise<JobPageDto> {
    return this.queryClient.fetchQuery({
      queryKey: jobKeys.page(cursor),
      queryFn: () => fetchJobPage(cursor),
      staleTime: 0,
      retry: false,
    });
  }

  private fetchSearch(query: string): Promise<JobSearchDto> {
    return this.queryClient.fetchQuery({
      queryKey: jobKeys.search(query),
      queryFn: () => searchJobs(query),
      staleTime: 0,
      retry: false,
    });
  }

  private fetchDetail(jobId: string): Promise<JobDetailDto> {
    return this.queryClient.fetchQuery({
      queryKey: jobKeys.detail(jobId),
      queryFn: () => fetchJob(jobId),
      staleTime: 0,
      retry: false,
    });
  }

  private patch(change: Partial<JobSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change };
    this.reportDirtyIfChanged();
    for (const listener of this.listeners) {
      listener();
    }
  }

  /**
   * Reports the dirty flag to the Rust close gate when it changed. The flag is derived from the
   * snapshot, so a save, a discard, a delete and an entity switch clear it through the same path.
   */
  private reportDirtyIfChanged(): void {
    const dirty = this.isDirty();
    if (dirty === this.reportedDirty) {
      return;
    }
    this.reportedDirty = dirty;
    void setShellDirty("job", dirty).catch(() => undefined);
  }

  /** Called when the page mounts: the close gate learns the current dirty state. */
  attach(): void {
    this.reportDirtyIfChanged();
  }

  /** Called when the page unmounts: the drafts are dropped, so the close gate must not keep refusing. */
  detach(): void {
    if (this.reportedDirty) {
      this.reportedDirty = false;
      void setShellDirty("job", false).catch(() => undefined);
    }
  }
}
