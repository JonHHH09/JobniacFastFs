import { CircleAlert, Plus } from "lucide-react";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { formatMessage, isMessageId, type MessageId } from "../../i18n/messages";
import type { DisagreementViewDto, MatchReportViewDto, ReviewViewDto } from "../../ipc/bindings";
import { isCommandError, unknownFailure } from "../../ipc/commands";
import { formatInstant, humanizeEnum } from "../../lib/format";
import { cn } from "../../lib/utils";
import { Badge } from "../../ui/badge";
import { Button } from "../../ui/button";
import { Field } from "../../ui/field";
import { Input } from "../../ui/input";
import { ListRow } from "../../ui/list-row";
import { ScoreMeter } from "../../ui/score-meter";
import { Select } from "../../ui/select";
import { StickyActions } from "../../ui/sticky-actions";
import { useMatchCapabilities } from "../matching/capabilities";
import { useOpenedReport } from "../matching/openedReport";
import { keyedRows } from "../matching/rows";
import { ReportPicker } from "../pickers";
import * as api from "./api";
import {
  COMPONENTS,
  type ComponentDetail,
  emptyDraft,
  FACTS,
  identityReady,
  MAX_CONCERN_ROWS,
  MAX_DETAIL_ROWS,
  MAX_ISSUE_LENGTH,
  MAX_LABEL_LENGTH,
  MAX_SCORE,
  OUTCOMES,
  type ReviewDraft,
  reportFilter,
  STATUSES,
  SUMMARIES,
  scoreValue,
  signature,
  submitRequest,
  wholeNumber,
} from "./reviewDraft";

/** The operation that holds the review workspace. Only one runs at a time (the Rust guard too). */
type Busy = "submit" | "reviews" | "review" | "disagreements" | "acknowledge" | "link" | null;

/** A catalogue message with its placeholder values. */
type Message = { id: MessageId; params?: Record<string, string | number> };

/** The last review list and the report filter it was loaded with (the next page keeps it). */
type ReviewList = { filter: string | null; reviews: ReviewViewDto[]; nextCursor: string | null };

/** The last disagreement list and the report filter it was loaded with. */
type DisagreementList = {
  filter: string | null;
  rows: DisagreementViewDto[];
  nextCursor: string | null;
};

/** The label of a wire value, or the empty string when the catalogue does not know it. */
function enumLabel(prefix: string, value: string): string {
  const id = `${prefix}.${value}`;
  return isMessageId(id) ? formatMessage(id) : "";
}

/** The label of a wire value, or the raw value when the catalogue does not know it. */
function nameOf(prefix: string, value: string): string {
  return enumLabel(prefix, value) || value;
}

/**
 * Maps a failed command to its copy. Only `not_found` has its own copy (the draft is kept). A busy
 * refusal has no copy of its own: the caller keeps the message it shows.
 */
function failureMessage(messageId: string): MessageId {
  if (messageId === "reviews.unconfigured") {
    return "reviews.unconfigured";
  }
  if (messageId === "reviews.error.notFound") {
    return "reviews.error.notFound";
  }
  return "reviews.error.generic";
}

/** The reasons of a disagreement in words, comma separated. The wire values stay in the data. */
function reasonsText(reasons: string[]): string {
  return reasons.map((reason) => humanizeEnum("disagreementReason", reason)).join(", ");
}

/** The Rust single-flight refusal of the review workspace. */
const REVIEW_BUSY = "reviews.busy";

/** An integer component point count from a field value, or null when the text is not an integer. */
function integerPoints(value: string): number | null {
  if (value.trim() === "") {
    return 0;
  }
  return wholeNumber(value);
}

/** The available points of a component in the opened report, or 0 when the report has no such row. */
function availableOf(view: MatchReportViewDto | null, component: string): number {
  return view?.report.components.find((row) => row.component === component)?.availablePoints ?? 0;
}

/**
 * The Advisory reviews screen: the review draft with its component details and evidence
 * concerns, save and discard, the saved reviews and disagreements, and the acknowledge and link
 * actions. The draft is kept when a request fails. Nothing is saved or contacted outside Rust.
 */
export function ReviewsPage() {
  const opened = useOpenedReport();
  const openedReport = opened.report;
  const [draft, setDraft] = useState<ReviewDraft>(emptyDraft);
  const [baseline, setBaseline] = useState(() => signature(emptyDraft()));
  const [busy, setBusy] = useState<Busy>(null);
  // Known at startup: null until the check answers. The query
  // is shared with the Matching screen, so Rust is asked once.
  const capabilities = useMatchCapabilities();
  // A command that answered "unconfigured" switches the screen off until it is opened again.
  const [refused, setRefused] = useState(false);
  // The check itself failed (an IPC error): not the same as an explicit "not configured" answer.
  const checkFailed = capabilities.failed;
  const configured: boolean | null =
    refused || checkFailed ? false : (capabilities.data?.reviews ?? null);
  const [message, setMessage] = useState<Message | null>(null);
  const [savedReview, setSavedReview] = useState<ReviewViewDto | null>(null);
  const [reviewList, setReviewList] = useState<ReviewList | null>(null);
  const [disagreementList, setDisagreementList] = useState<DisagreementList | null>(null);
  const [selected, setSelected] = useState<DisagreementViewDto | null>(null);
  const [issueText, setIssueText] = useState("");
  // Refs, not only state, so that two clicks before the next render cannot both start a request.
  const inFlight = useRef(false);
  const nextRowId = useRef(1);
  // The dirty flag last reported to Rust. Starts false, so nothing is sent until the draft changes.
  const reportedDirty = useRef(false);

  const unconfigured = configured === false;
  const locked = configured !== true || busy !== null;
  const dirty = signature(draft) !== baseline;

  // The Rust close gate refuses while the draft holds unsaved edits, so each change is reported.
  useEffect(() => {
    if (reportedDirty.current === dirty) {
      return;
    }
    reportedDirty.current = dirty;
    void api.setDraftDirty(dirty).catch(() => undefined);
  }, [dirty]);

  /** Runs one command under the single-flight guard. The result is published only on success. */
  async function run<T>(
    kind: Exclude<Busy, null>,
    task: () => Promise<T>,
    publish: (value: T) => void,
  ) {
    if (inFlight.current || configured !== true) {
      return;
    }
    inFlight.current = true;
    const shown = message;
    setBusy(kind);
    try {
      publish(await task());
    } catch (error) {
      const failure = isCommandError(error) ? error : unknownFailure;
      if (failure.messageId === REVIEW_BUSY) {
        // Another request holds the workspace: the message that was shown before stays.
        setMessage(shown);
        return;
      }
      if (failure.messageId === "reviews.unconfigured") {
        setRefused(true);
      }
      setMessage({ id: failureMessage(failure.messageId) });
    } finally {
      inFlight.current = false;
      setBusy(null);
    }
  }

  function update(patch: Partial<ReviewDraft>) {
    setDraft((previous) => ({ ...previous, ...patch }));
  }

  function newRowId(): number {
    const id = nextRowId.current;
    nextRowId.current += 1;
    return id;
  }

  function addDetail() {
    const first = openedReport?.report.components[0];
    if (!first || draft.details.length >= MAX_DETAIL_ROWS) {
      return;
    }
    update({
      details: [
        ...draft.details,
        {
          id: newRowId(),
          component: first.component,
          status: "UNKNOWN",
          earnedPoints: 0,
          availablePoints: first.availablePoints,
        },
      ],
    });
  }

  function changeDetail(id: number, patch: Partial<ComponentDetail>) {
    update({
      details: draft.details.map((row) => (row.id === id ? { ...row, ...patch } : row)),
    });
  }

  function addConcern() {
    if (draft.concerns.length >= MAX_CONCERN_ROWS) {
      return;
    }
    update({
      concerns: [
        ...draft.concerns,
        {
          id: newRowId(),
          component: COMPONENTS[0],
          status: "UNKNOWN",
          fact: FACTS[0],
          outcomeChangingDefect: false,
        },
      ],
    });
  }

  function save() {
    const request = submitRequest(draft);
    const snapshot = signature(draft);
    void run(
      "submit",
      () => api.submitReview(request),
      (result) => {
        setSavedReview(result.review);
        setSelected(result.disagreement ?? null);
        setIssueText(result.disagreement?.issueReference ?? "");
        setBaseline(snapshot);
        setMessage({ id: "reviews.saved" });
      },
    );
  }

  function discard() {
    const fresh = emptyDraft();
    setDraft(fresh);
    setBaseline(signature(fresh));
    setMessage(null);
  }

  function loadReviews(filter: string | null, cursor: string | null) {
    void run(
      "reviews",
      () => api.fetchReviews(filter, cursor),
      (page) => {
        setReviewList({ filter, reviews: page.reviews, nextCursor: page.nextCursor });
        setMessage(page.reviews.length === 0 ? { id: "reviews.list.empty" } : null);
      },
    );
  }

  function openReview(id: string) {
    void run(
      "review",
      () => api.fetchReview(id),
      (view) => {
        setSavedReview(view);
        setMessage(null);
      },
    );
  }

  function loadDisagreements(filter: string | null, cursor: string | null) {
    void run(
      "disagreements",
      () => api.fetchDisagreements(filter, cursor),
      (page) => {
        setDisagreementList({ filter, rows: page.disagreements, nextCursor: page.nextCursor });
        setMessage(page.disagreements.length === 0 ? { id: "reviews.disagreements.empty" } : null);
      },
    );
  }

  function openDisagreement(row: DisagreementViewDto) {
    setSelected(row);
    setIssueText(row.issueReference ?? "");
  }

  /** Publishes an acknowledged or linked disagreement in the selection and in the list. */
  function applyDisagreement(row: DisagreementViewDto) {
    setSelected(row);
    setDisagreementList((list) =>
      list ? { ...list, rows: list.rows.map((item) => (item.id === row.id ? row : item)) } : list,
    );
    setMessage(null);
  }

  function acknowledge() {
    if (!selected) {
      return;
    }
    const id = selected.id;
    const reference = issueText.trim() === "" ? null : issueText.trim();
    void run("acknowledge", () => api.acknowledgeDisagreement(id, reference), applyDisagreement);
  }

  function link() {
    if (!selected || issueText.trim() === "") {
      return;
    }
    const id = selected.id;
    const reference = issueText.trim();
    void run("link", () => api.linkDisagreement(id, reference), applyDisagreement);
  }

  const statusText = checkFailed
    ? formatMessage("reviews.checkFailed")
    : unconfigured
      ? formatMessage("reviews.unconfigured")
      : configured === null
        ? formatMessage("reviews.checking")
        : busy !== null
          ? formatMessage("reviews.working")
          : message
            ? formatMessage(message.id, message.params)
            : "";
  const isFailure = checkFailed || (!busy && (message?.id.startsWith("reviews.error.") ?? false));
  // Messages about the lists show in the lists panel, next to the buttons that cause them.
  const listStatus =
    !busy &&
    (message?.id === "reviews.list.empty" || message?.id === "reviews.disagreements.empty");
  const statusNode =
    statusText !== "" ? (
      <p
        role={isFailure ? "alert" : "status"}
        aria-live={isFailure ? "assertive" : "polite"}
        data-testid="reviewStatusLabel"
        className={cn(
          "flex min-w-0 flex-1 basis-60 items-start gap-2 [overflow-wrap:anywhere]",
          isFailure ? "text-destructive" : "text-muted-foreground",
        )}
      >
        {isFailure ? (
          <CircleAlert className="mt-0.5 size-4 shrink-0" strokeWidth={1.75} aria-hidden />
        ) : null}
        <span>{statusText}</span>
      </p>
    ) : null;
  const reportId = reportFilter(draft.reportId);
  const componentOptions = (openedReport?.report.components ?? []).map((row) => row.component);

  return (
    <div className="@container grid min-w-0 gap-4">
      <Panel
        id="review-editor"
        title={formatMessage("ui.review.section")}
        testId="reviewPanel"
        footer={
          <StickyActions className="rounded-b-panel px-6">
            <Button
              type="button"
              data-testid="submitMatchReviewButton"
              disabled={locked || !identityReady(draft)}
              onClick={save}
            >
              {formatMessage("ui.review.submit")}
            </Button>
            <Button
              type="button"
              variant="outline"
              data-testid="discardReviewDraftButton"
              disabled={locked || !dirty}
              onClick={discard}
            >
              {formatMessage("ui.review.discard")}
            </Button>
            {listStatus ? null : statusNode}
          </StickyActions>
        }
      >
        <p className="text-[13px] text-muted-foreground" data-testid="reviewAuthorityLabel">
          {formatMessage("reviews.authority")}
        </p>
        <div className="grid gap-x-8 gap-y-6 @[720px]:grid-cols-2">
          <div className="grid min-w-0 content-start gap-3">
            <div className="grid items-start gap-3 @[400px]:grid-cols-2">
              <Field
                id="review-report-id"
                label={formatMessage("ui.review.reportId")}
                className="@[400px]:col-span-2"
              >
                {(control) => (
                  <ReportPicker
                    {...control}
                    testId="reviewreportIdInput"
                    value={draft.reportId}
                    disabled={locked}
                    onChange={(value) => update({ reportId: value })}
                  />
                )}
              </Field>
              {openedReport !== null && draft.reportId.trim() !== openedReport.report.id ? (
                <div className="@[400px]:col-span-2">
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    disabled={locked}
                    onClick={() => update({ reportId: openedReport.report.id })}
                  >
                    {formatMessage("ui.review.useOpenedReport")}
                  </Button>
                </div>
              ) : null}
              <TextField
                id="review-reviewer"
                testId="reviewreviewerInput"
                label={formatMessage("ui.review.reviewer")}
                value={draft.reviewer}
                maxLength={MAX_LABEL_LENGTH}
                disabled={locked}
                onChange={(value) => update({ reviewer: value })}
              />
              <TextField
                id="review-model"
                testId="reviewmodelInput"
                label={formatMessage("ui.review.model")}
                value={draft.model}
                maxLength={MAX_LABEL_LENGTH}
                disabled={locked}
                onChange={(value) => update({ model: value })}
              />
              <TextField
                id="review-version"
                testId="reviewreviewVersionInput"
                label={formatMessage("ui.review.reviewVersion")}
                value={draft.reviewVersion}
                maxLength={MAX_LABEL_LENGTH}
                disabled={locked}
                onChange={(value) => update({ reviewVersion: value })}
              />
              <p className="text-[13px] text-muted-foreground @[400px]:col-span-2">
                {formatMessage("reviews.hint")}
              </p>
              <Field
                id="review-score"
                label={formatMessage("ui.review.score")}
                error={
                  scoreValue(draft.score) === null ? formatMessage("reviews.input.score") : null
                }
              >
                {(control) => (
                  <Input
                    {...control}
                    data-testid="reviewScoreInput"
                    type="number"
                    min={0}
                    max={100}
                    step={1}
                    className="tabular-nums"
                    value={draft.score}
                    disabled={locked}
                    aria-invalid={scoreValue(draft.score) === null}
                    onChange={(event) => {
                      // Out-of-range whole numbers are kept inside 0 to 100; empty or fractional text
                      // stays as typed and is flagged below.
                      const points = wholeNumber(event.target.value);
                      update({
                        score:
                          points === null
                            ? event.target.value
                            : String(Math.min(points, MAX_SCORE)),
                      });
                    }}
                  />
                )}
              </Field>
              <SelectField
                id="review-outcome"
                testId="reviewOutcomeInput"
                label={formatMessage("ui.review.outcome")}
                value={draft.outcome}
                disabled={locked}
                options={OUTCOMES.map((value) => ({
                  value,
                  label: nameOf("reviews.outcome", value),
                }))}
                onChange={(value) => update({ outcome: value })}
              />
              <label className="flex h-9 items-center gap-2 self-end">
                <input
                  type="checkbox"
                  data-testid="reviewBlockerInput"
                  className="size-4 accent-primary"
                  checked={draft.blockerMismatch}
                  disabled={locked}
                  onChange={(event) => update({ blockerMismatch: event.target.checked })}
                />
                {formatMessage("ui.review.blocker")}
              </label>
              <SelectField
                id="review-summary"
                testId="reviewSummaryInput"
                label={formatMessage("ui.review.summary")}
                value={draft.summary}
                disabled={locked}
                options={SUMMARIES.map((value) => ({
                  value,
                  label: nameOf("reviews.summary", value),
                }))}
                onChange={(value) => update({ summary: value })}
                className="@[400px]:col-span-2"
              />
            </div>
          </div>

          <div className="grid min-w-0 content-start gap-6">
            <section className="grid gap-3">
              <div className="flex flex-wrap items-start justify-between gap-2">
                <h3 className="text-sm leading-4 font-semibold">
                  {formatMessage("ui.review.details")}
                </h3>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="-mt-1.5 -mb-1.5"
                  data-testid="addComponentDetailButton"
                  disabled={
                    locked || openedReport === null || draft.details.length >= MAX_DETAIL_ROWS
                  }
                  onClick={addDetail}
                >
                  <Plus className="size-4" strokeWidth={1.75} aria-hidden />
                  {formatMessage("ui.review.addDetail")}
                </Button>
              </div>
              {openedReport === null ? (
                <p className="text-[13px] text-muted-foreground">
                  {formatMessage("reviews.componentHint")}
                </p>
              ) : null}
              {draft.details.map((row) => (
                <div
                  key={row.id}
                  className="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)] gap-3 rounded-md border border-border p-3"
                >
                  <SelectField
                    id={`detail-component-${row.id}`}
                    testId="reviewComponentInput"
                    label={formatMessage("ui.review.detailComponent")}
                    value={row.component}
                    disabled={locked}
                    options={componentOptions.map((value) => ({
                      value,
                      label: nameOf("reviews.component", value),
                    }))}
                    onChange={(value) => {
                      // The earned points follow the new weight, so a draft never exceeds its maximum.
                      const available = availableOf(openedReport, value);
                      changeDetail(row.id, {
                        component: value,
                        availablePoints: available,
                        earnedPoints: Math.min(row.earnedPoints, available),
                      });
                    }}
                  />
                  <SelectField
                    id={`detail-status-${row.id}`}
                    testId="reviewComponentStatusInput"
                    label={formatMessage("ui.review.detailStatus")}
                    value={row.status}
                    disabled={locked}
                    options={STATUSES.map((value) => ({
                      value,
                      label: nameOf("reviews.status", value),
                    }))}
                    onChange={(value) => changeDetail(row.id, { status: value })}
                  />
                  <TextField
                    id={`detail-earned-${row.id}`}
                    testId="componentEarnedInput"
                    label={formatMessage("ui.review.detailEarned")}
                    type="number"
                    min={0}
                    max={row.availablePoints}
                    step={1}
                    value={String(row.earnedPoints)}
                    disabled={locked}
                    onChange={(value) => {
                      // Only whole points are kept (the core refuses a fraction); the range is 0 to the weight.
                      const points = integerPoints(value);
                      if (points !== null) {
                        changeDetail(row.id, {
                          earnedPoints: Math.min(Math.max(points, 0), row.availablePoints),
                        });
                      }
                    }}
                  />
                  <div className="flex items-end justify-end">
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      disabled={locked}
                      aria-label={formatMessage("ui.review.removeDetailNamed", {
                        index: draft.details.indexOf(row) + 1,
                        component: nameOf("reviews.component", row.component),
                      })}
                      onClick={() =>
                        update({ details: draft.details.filter((item) => item.id !== row.id) })
                      }
                    >
                      {formatMessage("ui.review.removeDetail")}
                    </Button>
                  </div>
                </div>
              ))}
            </section>

            <section className="grid gap-3">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <h3 className="text-sm font-semibold">{formatMessage("ui.review.concernGroup")}</h3>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  data-testid="addEvidenceConcernButton"
                  disabled={locked || draft.concerns.length >= MAX_CONCERN_ROWS}
                  onClick={addConcern}
                >
                  <Plus className="size-4" strokeWidth={1.75} aria-hidden />
                  {formatMessage("ui.review.addDefect")}
                </Button>
              </div>
              {draft.concerns.map((row) => (
                <div
                  key={row.id}
                  className="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)] gap-3 rounded-md border border-border p-3"
                >
                  <SelectField
                    id={`concern-component-${row.id}`}
                    testId="defectComponentInput"
                    label={formatMessage("ui.review.defectComponent")}
                    value={row.component}
                    disabled={locked}
                    options={COMPONENTS.map((value) => ({
                      value,
                      label: nameOf("reviews.component", value),
                    }))}
                    onChange={(value) =>
                      update({
                        concerns: draft.concerns.map((item) =>
                          item.id === row.id ? { ...item, component: value } : item,
                        ),
                      })
                    }
                  />
                  <SelectField
                    id={`concern-status-${row.id}`}
                    testId="defectStatusInput"
                    label={formatMessage("ui.review.defectStatus")}
                    value={row.status}
                    disabled={locked}
                    options={STATUSES.map((value) => ({
                      value,
                      label: nameOf("reviews.status", value),
                    }))}
                    onChange={(value) =>
                      update({
                        concerns: draft.concerns.map((item) =>
                          item.id === row.id ? { ...item, status: value } : item,
                        ),
                      })
                    }
                  />
                  <SelectField
                    id={`concern-fact-${row.id}`}
                    testId="defectFactInput"
                    label={formatMessage("ui.review.defectFact")}
                    value={row.fact}
                    disabled={locked}
                    options={FACTS.map((value) => ({
                      value,
                      label: humanizeEnum("evidenceDefect", value),
                    }))}
                    onChange={(value) =>
                      update({
                        concerns: draft.concerns.map((item) =>
                          item.id === row.id ? { ...item, fact: value } : item,
                        ),
                      })
                    }
                    className="col-span-2"
                  />
                  <label className="flex items-center gap-2">
                    <input
                      type="checkbox"
                      data-testid="outcomeChangingInput"
                      className="size-4 accent-primary"
                      checked={row.outcomeChangingDefect}
                      disabled={locked}
                      onChange={(event) =>
                        update({
                          concerns: draft.concerns.map((item) =>
                            item.id === row.id
                              ? { ...item, outcomeChangingDefect: event.target.checked }
                              : item,
                          ),
                        })
                      }
                    />
                    {formatMessage("ui.review.defectOutcome")}
                  </label>
                  <div className="flex items-center justify-end">
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      disabled={locked}
                      aria-label={formatMessage("ui.review.removeConcernNamed", {
                        index: draft.concerns.indexOf(row) + 1,
                      })}
                      onClick={() =>
                        update({ concerns: draft.concerns.filter((item) => item.id !== row.id) })
                      }
                    >
                      {formatMessage("ui.review.removeDetail")}
                    </Button>
                  </div>
                </div>
              ))}
            </section>
          </div>
        </div>
      </Panel>

      {unconfigured ? null : (
        <Panel id="review-lists" title={formatMessage("ui.review.listsHeading")}>
          <div data-testid="reviewListsPanel" className="grid gap-4">
            <div className="flex flex-wrap items-center gap-2">
              <Button
                type="button"
                variant="outline"
                data-testid="loadReviewHistoryButton"
                disabled={locked}
                onClick={() => loadReviews(reportId, null)}
              >
                {formatMessage("ui.review.loadReviews")}
              </Button>
              <Button
                type="button"
                variant="outline"
                data-testid="loadDisagreementHistoryButton"
                disabled={locked}
                onClick={() => loadDisagreements(reportId, null)}
              >
                {formatMessage("ui.review.loadDisagreements")}
              </Button>
            </div>
            {listStatus ? statusNode : null}
            {reviewList !== null && reviewList.reviews.length > 0 ? (
              <h3 className="text-sm font-semibold">{formatMessage("ui.review.reviewsHeading")}</h3>
            ) : null}
            {reviewList !== null && reviewList.reviews.length > 0 ? (
              <ul className="divide-y divide-border overflow-hidden rounded-md border border-border">
                {reviewList.reviews.map((review, position) => (
                  <li key={review.id}>
                    <ListRow
                      selected={review.id === savedReview?.id}
                      title={formatMessage("ui.review.rowTitle", {
                        reviewer: review.reviewer,
                        version: review.reviewVersion,
                      })}
                      meta={
                        <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
                          <time
                            dateTime={review.createdAt}
                            className="[font-variant-numeric:normal]"
                          >
                            {formatInstant(review.createdAt)}
                          </time>
                          {enumLabel("reviews.outcome", review.outcome) !== "" ? (
                            <Badge variant="outline" title={review.outcome}>
                              {enumLabel("reviews.outcome", review.outcome)}
                            </Badge>
                          ) : null}
                        </span>
                      }
                      trailing={
                        <>
                          <ScoreMeter
                            value={review.overallScore}
                            label={formatMessage("reviews.row.review", {
                              reviewer: review.reviewer,
                              overallScore: review.overallScore,
                            })}
                            className="hidden w-40 @[560px]:flex"
                          />
                          <span className="font-semibold tabular-nums @[560px]:hidden">
                            {review.overallScore}
                          </span>
                        </>
                      }
                      data-testid="openReviewButton"
                      aria-label={formatMessage("ui.review.openReviewNamed", {
                        index: position + 1,
                      })}
                      disabled={locked}
                      onSelect={() => openReview(review.id)}
                    />
                  </li>
                ))}
              </ul>
            ) : null}
            {reviewList?.nextCursor ? (
              <div>
                <Button
                  type="button"
                  variant="outline"
                  data-testid="nextReviewsPageButton"
                  disabled={locked}
                  onClick={() => loadReviews(reviewList.filter, reviewList.nextCursor)}
                >
                  {formatMessage("ui.review.nextReviews")}
                </Button>
              </div>
            ) : null}
            {disagreementList !== null && disagreementList.rows.length > 0 ? (
              <h3 className="text-sm font-semibold">
                {formatMessage("ui.review.disagreementsHeading")}
              </h3>
            ) : null}
            {disagreementList !== null && disagreementList.rows.length > 0 ? (
              <ul className="divide-y divide-border overflow-hidden rounded-md border border-border">
                {disagreementList.rows.map((row, position) => (
                  <li key={row.id}>
                    <ListRow
                      selected={row.id === selected?.id}
                      title={reasonsText(row.reasons)}
                      meta={
                        <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
                          <Badge variant="outline" title={row.status}>
                            {humanizeEnum("disagreementStatus", row.status)}
                          </Badge>
                          <time dateTime={row.createdAt} className="[font-variant-numeric:normal]">
                            {formatInstant(row.createdAt)}
                          </time>
                        </span>
                      }
                      data-testid="openDisagreementButton"
                      aria-label={formatMessage("ui.review.openDisagreementNamed", {
                        index: position + 1,
                      })}
                      disabled={locked}
                      onSelect={() => openDisagreement(row)}
                    />
                  </li>
                ))}
              </ul>
            ) : null}
            {disagreementList?.nextCursor ? (
              <div>
                <Button
                  type="button"
                  variant="outline"
                  data-testid="nextDisagreementsPageButton"
                  disabled={locked}
                  onClick={() =>
                    loadDisagreements(disagreementList.filter, disagreementList.nextCursor)
                  }
                >
                  {formatMessage("ui.review.nextDisagreements")}
                </Button>
              </div>
            ) : null}
          </div>
        </Panel>
      )}

      {unconfigured || savedReview === null ? null : <SavedReviewPanel review={savedReview} />}

      {unconfigured || selected === null ? null : (
        <Panel
          id="selected-disagreement"
          title={formatMessage("ui.review.selectedDisagreement")}
          testId="disagreementPanel"
        >
          <div className="grid gap-1.5 [overflow-wrap:anywhere]">
            <p data-testid="disagreementStatusLabel">
              <Badge variant="outline" title={selected.status}>
                {humanizeEnum("disagreementStatus", selected.status)}
              </Badge>
            </p>
            <p data-testid="disagreementReasonsLabel" className="font-semibold">
              {reasonsText(selected.reasons)}
            </p>
            <p data-testid="disagreementDefectCodesLabel" className="text-muted-foreground">
              {selected.evidenceDefectCodes
                .map((code) => humanizeEnum("evidenceDefect", code))
                .join(", ")}
            </p>
          </div>
          <p className="text-[13px] text-muted-foreground">{formatMessage("reviews.linkHint")}</p>
          <TextField
            id="existing-issue-reference"
            label={formatMessage("ui.review.issueReference")}
            testId="existingIssueReferenceInput"
            value={issueText}
            maxLength={MAX_ISSUE_LENGTH}
            disabled={locked}
            onChange={setIssueText}
          />
          <div className="flex flex-wrap items-center gap-2">
            <Button
              type="button"
              variant="outline"
              data-testid="acknowledgeMatchDisagreementButton"
              disabled={locked}
              onClick={acknowledge}
            >
              {formatMessage("ui.review.acknowledge")}
            </Button>
            <Button
              type="button"
              data-testid="linkMatchDisagreementButton"
              disabled={locked || issueText.trim() === ""}
              onClick={link}
            >
              {formatMessage("ui.review.link")}
            </Button>
          </div>
          {issueText.trim() === "" ? (
            <p className="text-[13px] text-muted-foreground" data-testid="reviewIssueHint">
              {formatMessage("reviews.input.issueRef")}
            </p>
          ) : null}
        </Panel>
      )}
    </div>
  );
}

/**
 * A titled group of the screen: a surface with a rule and a labelled region. Unlike the shared
 * section panel it does not clip its content, so the footer can stick to the bottom of the window.
 */
function Panel({
  id,
  title,
  testId,
  footer,
  children,
}: {
  id: string;
  title: string;
  testId?: string;
  footer?: ReactNode;
  children: ReactNode;
}) {
  const headingId = `${id}-heading`;
  return (
    <section
      aria-labelledby={headingId}
      data-testid={testId}
      className="min-w-0 rounded-panel border border-border bg-card text-card-foreground"
    >
      <div className="grid min-w-0 gap-4 px-6 py-5">
        <h2 id={headingId} className="text-base leading-snug font-semibold">
          {title}
        </h2>
        {children}
      </div>
      {footer}
    </section>
  );
}

/** The badge look of a component status. Colour only supports the label. */
function statusVariant(status: string): "success" | "warning" | "destructive" | "neutral" {
  if (status === "MATCH") {
    return "success";
  }
  if (status === "PARTIAL") {
    return "warning";
  }
  return status === "MISMATCH" ? "destructive" : "neutral";
}

/** The share of the available points a component earned, as a whole number from 0 to 100. */
function percentOf(earned: number, available: number): number {
  return available > 0 ? Math.round((earned / available) * 100) : 0;
}

/** The saved advisory review: its heading, model and outcome, the summary, components and evidence. */
function SavedReviewPanel({ review }: { review: ReviewViewDto }) {
  const summary = enumLabel("reviews.summary", review.summary);
  const heading = formatMessage("reviews.review.version", {
    reviewer: review.reviewer,
    reviewVersion: review.reviewVersion,
  });
  const outcome = enumLabel("reviews.outcome", review.outcome);
  return (
    <Panel
      id="saved-review"
      title={formatMessage("ui.review.savedReview")}
      testId="savedReviewPanel"
    >
      <div className="grid gap-2">
        <ScoreMeter
          value={review.overallScore}
          label={formatMessage("reviews.row.review", {
            reviewer: review.reviewer,
            overallScore: review.overallScore,
          })}
          className="max-w-md [&_[data-tabular]]:w-14 [&_[data-tabular]]:text-3xl"
        />
        <p className="font-semibold">{heading}</p>
        <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[13px] text-muted-foreground">
          <span>{formatMessage("reviews.review.modelOnly", { model: review.model })}</span>
          {outcome !== "" ? (
            <Badge variant="outline" title={review.outcome}>
              {outcome}
            </Badge>
          ) : null}
          <time dateTime={review.createdAt} className="[font-variant-numeric:normal]">
            {formatInstant(review.createdAt)}
          </time>
        </p>
        {summary !== "" ? <p>{summary}</p> : null}
        {review.blockerMismatch ? (
          <p className="flex items-start gap-2 text-destructive">
            <CircleAlert className="mt-0.5 size-4 shrink-0" strokeWidth={1.75} aria-hidden />
            <span>{formatMessage("reviews.review.blocker")}</span>
          </p>
        ) : null}
      </div>
      <div className="grid gap-x-8 gap-y-5 @[720px]:grid-cols-2">
        <div className="grid min-w-0 content-start gap-3">
          {review.componentsTruncated ? (
            <p className="text-[13px]">
              {formatMessage("reviews.review.componentsTruncated", {
                shown: review.components.length,
                total: review.componentsTotalCount,
              })}
            </p>
          ) : null}
          <ul className="grid gap-3">
            {keyedRows(review.components).map(({ key, row }) => {
              const name = humanizeEnum("matchComponent", row.component);
              return (
                <li key={key} className="grid gap-1.5">
                  <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1">
                    <span className="text-[13px] font-medium [overflow-wrap:anywhere]">{name}</span>
                    <span className="flex items-center gap-2">
                      <span className="text-[13px] tabular-nums text-muted-foreground">
                        {formatMessage("matching.report.points", {
                          earned: row.earnedPoints,
                          available: row.availablePoints,
                        })}
                      </span>
                      <Badge variant={statusVariant(row.status)} title={row.status}>
                        {humanizeEnum("matchStatus", row.status)}
                      </Badge>
                    </span>
                  </div>
                  <ScoreMeter
                    value={percentOf(row.earnedPoints, row.availablePoints)}
                    label={name}
                    showValue={false}
                    className="[&_[role=meter]]:h-2"
                  />
                </li>
              );
            })}
          </ul>
        </div>
        <div className="grid min-w-0 content-start gap-3">
          {review.evidenceTruncated ? (
            <p className="text-[13px]">
              {formatMessage("reviews.review.evidenceTruncated", {
                shown: review.evidence.length,
                total: review.evidenceTotalCount,
              })}
            </p>
          ) : null}
          <ul className="grid gap-3">
            {keyedRows(review.evidence).map(({ key, row }) => (
              <li key={key} className="grid gap-0.5 text-[13px] [overflow-wrap:anywhere]">
                <span>{humanizeEnum("evidenceDefect", row.fact)}</span>
                <span className="flex flex-wrap items-center gap-x-2 text-xs text-muted-foreground">
                  <span>{humanizeEnum("matchComponent", row.component)}</span>
                  <span aria-hidden className="h-3 w-px bg-border" />
                  <span>{humanizeEnum("matchStatus", row.status)}</span>
                </span>
              </li>
            ))}
          </ul>
        </div>
      </div>
    </Panel>
  );
}

/** A labelled text input of the review form. */
function TextField({
  id,
  label,
  value,
  onChange,
  disabled,
  maxLength,
  type,
  min,
  max,
  step,
  testId,
  className,
  hint,
}: {
  id: string;
  hint?: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
  disabled: boolean;
  maxLength?: number;
  type?: string;
  min?: number;
  max?: number;
  step?: number;
  testId?: string;
  className?: string;
}) {
  return (
    <Field id={id} label={label} hint={hint} className={className}>
      {(control) => (
        <Input
          {...control}
          data-testid={testId}
          type={type}
          min={min}
          max={max}
          step={step}
          maxLength={maxLength}
          value={value}
          disabled={disabled}
          onChange={(event) => onChange(event.target.value)}
        />
      )}
    </Field>
  );
}

/** A labelled select of wire values with their copy. */
function SelectField({
  id,
  label,
  value,
  options,
  onChange,
  disabled,
  testId,
  className,
}: {
  id: string;
  label: string;
  value: string;
  options: { value: string; label: string }[];
  onChange: (value: string) => void;
  disabled: boolean;
  testId?: string;
  className?: string;
}) {
  return (
    <Field id={id} label={label} className={className}>
      {(control) => (
        <Select
          {...control}
          data-testid={testId}
          value={value}
          disabled={disabled}
          onChange={(event) => onChange(event.target.value)}
        >
          {options.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </Select>
      )}
    </Field>
  );
}
