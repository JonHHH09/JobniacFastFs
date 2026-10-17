import { Check, FileSearch, Info } from "lucide-react";
import { useEffect, useState } from "react";
import { formatMessage, type MessageId } from "../../i18n/messages";
import type {
  CandidateDto,
  IngestResultDto,
  IngestStatus,
  MatchedOn,
  SourceViewDto,
} from "../../ipc/bindings";
import { isCommandError, unknownFailure } from "../../ipc/commands";
import { formatInstant, formatRelative } from "../../lib/format";
import { cn } from "../../lib/utils";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "../../ui/alert-dialog";
import { Badge } from "../../ui/badge";
import { Button } from "../../ui/button";
import { Field } from "../../ui/field";
import { Input } from "../../ui/input";
import { Label } from "../../ui/label";
import { Marker } from "../../ui/marker";
import { Separator } from "../../ui/separator";
import { Spinner } from "../../ui/spinner";
import { StatusMessage } from "../../ui/status-message";
import { Textarea } from "../../ui/textarea";
import { previewStoredDocument } from "../documents/api";
import { ProfilePicker, StoredDocumentPicker } from "../pickers";
import * as api from "./api";

/** The operation that holds the workspace. Only one runs at a time (the Rust guard too). */
type Busy = "ingest" | "confirm" | "cancel" | "lookup" | "review" | null;

/** The text of the stored PDF as the review step shows it (untrusted plain text). */
type Review = { text: string; truncated: boolean };

/** How a status line reads: failures and refusals are errors, outcomes are successes. */
function toneOf(id: MessageId): "error" | "neutral" | "success" {
  if (id.startsWith("profilePdf.done.")) {
    return "success";
  }
  if (
    id === "profilePdf.cancelled" ||
    id.startsWith("profilePdf.progress.") ||
    id === "profilePdf.busy"
  ) {
    return "neutral";
  }
  return "error";
}

/** A catalogue message with its placeholder values. */
export type Message = { id: MessageId; params?: Record<string, string | number> };

/** The replacement staged by a confirmation step: the token Rust holds and the copy inputs. */
export type PendingReplacement = {
  token: string;
  existingProfileId: string;
  expectedRevision: string;
  documentId: string;
};

/** The field length limit of every ID input. */
export const ID_MAX_LENGTH = 36;

/** The copy id of each ingestion status. Rust sends the enum value; React never builds copy from text. */
const STATUS_MESSAGE: Record<IngestStatus, MessageId> = {
  createdProfile: "profilePdf.status.createdProfile",
  updatedProfile: "profilePdf.status.updatedProfile",
  reusedExistingSource: "profilePdf.status.reusedExistingSource",
  duplicateProfileCandidate: "profilePdf.status.duplicateProfileCandidate",
  ambiguousProfileCandidates: "profilePdf.status.ambiguousProfileCandidates",
};

/** The copy id of each matched-on reason. */
const MATCH_MESSAGE: Record<MatchedOn, MessageId> = {
  email: "profilePdf.match.email",
  github: "profilePdf.match.github",
  linkedin: "profilePdf.match.linkedin",
  otherLink: "profilePdf.match.otherLink",
  profileId: "profilePdf.match.profileId",
};

/**
 * Maps a failed command to its copy. The input refusals, the pending confirmation and the busy guard
 * have their own copy; every other failure is the generic one, and the entered values are kept.
 */
export function failureMessage(messageId: string): Message {
  const known: MessageId[] = [
    "profilePdf.intro.unavailable",
    "profilePdf.input.ids",
    "profilePdf.input.revision",
    "profilePdf.input.maxCharacters",
    "profilePdf.input.replacementRequired",
    "profilePdf.input.lookupId",
    "profilePdf.confirm.required",
    "profilePdf.busy",
  ];
  const id = known.find((candidate) => candidate === messageId);
  return { id: id ?? "profilePdf.error.generic" };
}

/** A text input as the request takes it: blank is absent (`null`). */
export function nullIfBlank(text: string): string | null {
  const trimmed = text.trim();
  return trimmed.length === 0 ? null : trimmed;
}

/**
 * The summary lines of an ingestion result, in display order. The status line comes first; a
 * candidate adds its profile, its matched reasons and the confirmation need; a written profile adds its
 * IDs. An already-linked candidate replaces the status line; the stored-document line is always kept.
 */
export function resultLines(result: IngestResultDto): Message[] {
  const lines: Message[] = [{ id: STATUS_MESSAGE[result.status] }];
  const candidate: CandidateDto | null = result.candidate;
  const linked = candidate?.alreadyLinked === true;
  if (linked) {
    lines[0] = { id: "profilePdf.summary.alreadyLinked" };
  }
  lines.push({ id: "profilePdf.summary.documentId", params: { documentId: result.documentId } });
  if (candidate !== null) {
    if (candidate.profileId !== null) {
      lines.push({
        id: "profilePdf.summary.candidateId",
        params: { profileId: candidate.profileId },
      });
    }
    if (candidate.matchedOn.length > 0) {
      const labels = candidate.matchedOn
        .map((reason) => formatMessage(MATCH_MESSAGE[reason]))
        .join(", ");
      lines.push({ id: "profilePdf.summary.matchedOn", params: { labels } });
    }
  }
  if (candidate !== null) {
    lines.push({ id: "profilePdf.summary.notCreated" });
  }
  if (candidate !== null && !linked) {
    lines.push({ id: "profilePdf.summary.needsConfirmation" });
  }
  if (result.profileId !== null) {
    lines.push({ id: "profilePdf.summary.profileId", params: { profileId: result.profileId } });
  }
  if (result.extractionId !== null) {
    lines.push({
      id: "profilePdf.summary.extractionId",
      params: { extractionId: result.extractionId },
    });
  }
  if (result.sourceLinkId !== null) {
    lines.push({
      id: "profilePdf.summary.sourceLinkId",
      params: { sourceLinkId: result.sourceLinkId },
    });
  }
  return lines;
}

/** The identifier lines of a profile's PDF source: raw values, shown only in a details block. */
export function sourceIdLines(source: SourceViewDto): string[] {
  return [
    formatMessage("profilePdf.source.sourceLinkId", { sourceLinkId: source.sourceLinkId }),
    formatMessage("profilePdf.source.profileId", { profileId: source.profileId }),
    formatMessage("profilePdf.source.extractionId", { extractionId: source.extractionId }),
  ];
}

/** The readable lines of a profile's PDF source: the source type and when it was created. */
export function sourceLines(source: SourceViewDto): string[] {
  const relative = formatRelative(source.createdAt);
  const created = formatInstant(source.createdAt);
  return [
    formatMessage("profilePdf.source.type"),
    formatMessage("profilePdf.source.createdAt", {
      createdAt: relative === "" ? created : `${created} (${relative})`,
    }),
  ];
}

const DETAIL_IDS: ReadonlySet<MessageId> = new Set<MessageId>([
  "profilePdf.summary.documentId",
  "profilePdf.summary.candidateId",
  "profilePdf.summary.profileId",
  "profilePdf.summary.extractionId",
  "profilePdf.summary.sourceLinkId",
]);

/** Whether the replacement checkbox needs its two companion fields before the button is enabled. */
export function replacementReady(
  overwrite: boolean,
  existingProfileId: string,
  expectedRevision: string,
): boolean {
  return (
    !overwrite ||
    (nullIfBlank(existingProfileId) !== null && nullIfBlank(expectedRevision) !== null)
  );
}

/**
 * The profile-from-stored-PDF screen. An ingestion either runs
 * or stages a replacement that the confirmation dialog must accept. The entered values are kept on a
 * failure; the Rust side holds the staged replacement and its token.
 */
export function ProfilePdfPage() {
  const [documentId, setDocumentId] = useState("");
  const [existingProfileId, setExistingProfileId] = useState("");
  const [overwrite, setOverwrite] = useState(false);
  const [expectedRevision, setExpectedRevision] = useState("");
  const [maxCharacters, setMaxCharacters] = useState("");
  const [lookupId, setLookupId] = useState("");
  const [busy, setBusy] = useState<Busy>(null);
  const [pending, setPending] = useState<PendingReplacement | null>(null);
  const [result, setResult] = useState<IngestResultDto | null>(null);
  const [source, setSource] = useState<SourceViewDto | null>(null);
  const [review, setReview] = useState<Review | null>(null);
  const [message, setMessage] = useState<Message | null>(null);
  const [unavailable, setUnavailable] = useState(false);

  // Rust knows before any click whether a database is configured: the unavailable intro and the disabled
  // actions show on mount. A failed check changes nothing; the failure-driven path in `fail` still applies.
  useEffect(() => {
    let active = true;
    api.fetchProfilePdfCapabilities().then(
      (capabilities) => {
        if (active && capabilities?.profilePdf === false) {
          setUnavailable(true);
        }
      },
      () => {},
    );
    return () => {
      active = false;
    };
  }, []);

  const editable = busy === null && pending === null && !unavailable;
  const canIngest =
    editable &&
    nullIfBlank(documentId) !== null &&
    replacementReady(overwrite, existingProfileId, expectedRevision);
  const canLookup = editable && nullIfBlank(lookupId) !== null;

  /** Records a failure: the message only; the entered values and the previous results stay. */
  function fail(error: unknown) {
    const failure = isCommandError(error) ? error : unknownFailure;
    if (failure.messageId === "profilePdf.intro.unavailable") {
      // The intro already shows this copy; the status line stays empty.
      setUnavailable(true);
      setMessage(null);
    } else {
      setMessage(failureMessage(failure.messageId));
    }
  }

  /** Another PDF was picked: the text read from the previous one no longer applies. */
  function onDocumentChange(next: string) {
    setDocumentId(next);
    setReview(null);
  }

  /** Reads the stored PDF's text without keeping it, so it can be checked before a profile is made. */
  async function previewText() {
    setBusy("review");
    setReview(null);
    setMessage({ id: "profilePdf.progress.review" });
    try {
      const stored = await previewStoredDocument(documentId.trim(), false);
      setReview({ text: stored.extraction.text, truncated: stored.extraction.truncated });
      setMessage(null);
    } catch (error) {
      const failure = isCommandError(error) ? error : unknownFailure;
      if (failure.messageId === "profilePdf.intro.unavailable") {
        setUnavailable(true);
        setMessage(null);
      } else {
        setMessage({ id: "profilePdf.review.failed" });
      }
    } finally {
      setBusy(null);
    }
  }

  async function ingest() {
    setBusy("ingest");
    setMessage({ id: "profilePdf.progress.ingest" });
    try {
      const step = await api.ingestProfileFromPdf({
        documentId: documentId.trim(),
        existingProfileId: nullIfBlank(existingProfileId),
        overwrite,
        expectedRevision: nullIfBlank(expectedRevision),
        maxCharacters: nullIfBlank(maxCharacters),
      });
      if (step.kind === "confirmationRequired") {
        setPending({
          token: step.token,
          existingProfileId: step.existingProfileId,
          expectedRevision: step.expectedRevision,
          documentId: step.documentId,
        });
        setMessage(null);
      } else {
        setResult(step.result);
        setMessage({ id: "profilePdf.done.result" });
      }
    } catch (error) {
      fail(error);
    } finally {
      setBusy(null);
    }
  }

  async function confirm() {
    if (pending === null) {
      return;
    }
    setBusy("confirm");
    try {
      const confirmed = await api.confirmReplacement(pending.token);
      setResult(confirmed);
      setMessage({ id: "profilePdf.done.result" });
    } catch (error) {
      fail(error);
    } finally {
      // The token is consumed by Rust whatever the outcome, so the dialog closes.
      setPending(null);
      setBusy(null);
    }
  }

  async function cancel() {
    if (pending === null) {
      return;
    }
    setBusy("cancel");
    try {
      await api.cancelReplacement(pending.token);
    } catch {
      // A failed cancel still leaves the dialog closed; Rust drops the token on the next request.
    } finally {
      setPending(null);
      setBusy(null);
      setMessage({ id: "profilePdf.cancelled" });
    }
  }

  async function lookup() {
    setBusy("lookup");
    setMessage({ id: "profilePdf.progress.source" });
    try {
      setSource(await api.lookupProfilePdfSource(lookupId.trim()));
      setMessage({ id: "profilePdf.done.source" });
    } catch (error) {
      fail(error);
    } finally {
      setBusy(null);
    }
  }

  const hasDocument = nullIfBlank(documentId) !== null;
  // The sequence: pick, review (optional, never blocks), create. The current step is where to go next.
  const steps: Array<{ label: MessageId; done: boolean; optional?: boolean }> = [
    { label: "profilePdf.step.pick", done: hasDocument },
    { label: "profilePdf.step.review", done: review !== null, optional: true },
    { label: "profilePdf.step.create", done: result !== null },
  ];
  const currentStep = result !== null ? -1 : !hasDocument ? 0 : review === null ? 1 : 2;

  return (
    <section
      aria-labelledby="profilePdf-heading"
      data-testid="profile-pdf-section"
      className="@container grid w-full min-w-0 gap-4"
    >
      <h2 id="profilePdf-heading" className="sr-only">
        {formatMessage("profilePdf.title")}
      </h2>
      {unavailable ? (
        <p
          className="flex items-start gap-2 rounded-panel border border-border bg-card px-4 py-3 text-muted-foreground"
          role="note"
        >
          <Info className="mt-0.5 size-4 shrink-0" strokeWidth={1.75} aria-hidden />
          <span>{formatMessage("profilePdf.intro.unavailable")}</span>
        </p>
      ) : null}

      <ol
        aria-label={formatMessage("profilePdf.stepper.label")}
        className="flex flex-wrap gap-x-6 gap-y-2"
      >
        {steps.map((step, index) => (
          <li
            key={step.label}
            aria-current={index === currentStep ? "step" : undefined}
            className="flex items-center gap-2 text-[13px]"
          >
            <span
              className={cn(
                "grid size-6 shrink-0 place-items-center rounded-full border text-xs font-semibold tabular-nums",
                step.done
                  ? "border-primary bg-primary text-primary-foreground"
                  : "border-input text-muted-foreground",
              )}
            >
              {step.done ? (
                <>
                  <Check className="size-3.5" strokeWidth={2} aria-hidden />
                  <span className="sr-only">{formatMessage("profilePdf.step.done")}</span>
                </>
              ) : (
                index + 1
              )}
            </span>
            {index === currentStep ? (
              <Marker className="font-semibold">{formatMessage(step.label)}</Marker>
            ) : (
              <span className={step.done ? "font-medium" : "text-muted-foreground"}>
                {formatMessage(step.label)}
              </span>
            )}
          </li>
        ))}
      </ol>

      <div className="min-w-0 rounded-panel border border-border bg-card">
        <div className="grid gap-3 p-4">
          <div className="grid gap-0.5">
            <h3 className="text-base font-semibold">{formatMessage("profilePdf.step.pick")}</h3>
            <p className="text-[13px] text-muted-foreground">
              {formatMessage("profilePdf.step.pick.help")}
            </p>
          </div>
          <Field label={formatMessage("profilePdf.label.documentId")} id="profilePdfDocumentId">
            {(control) => (
              <StoredDocumentPicker
                id={control.id}
                label={formatMessage("profilePdf.id.a11y")}
                value={documentId}
                onChange={onDocumentChange}
                disabled={!editable}
              />
            )}
          </Field>
        </div>

        <Separator />

        <div className="grid gap-3 p-4">
          <div className="grid gap-0.5">
            <h3 className="flex items-center gap-2 text-base font-semibold">
              {formatMessage("profilePdf.step.review")}
              <Badge variant="outline">{formatMessage("profilePdf.step.optional")}</Badge>
            </h3>
            <p className="text-[13px] text-muted-foreground">
              {formatMessage("profilePdf.step.review.help")}
            </p>
          </div>
          <div className="flex flex-wrap items-end gap-3">
            <Button
              type="button"
              id="reviewProfilePdfButton"
              variant="outline"
              disabled={!editable || !hasDocument}
              onClick={() => void previewText()}
            >
              <FileSearch strokeWidth={1.75} aria-hidden />
              {formatMessage("profilePdf.button.review")}
            </Button>
          </div>
          {review !== null ? (
            <div className="grid gap-1.5" data-testid="profile-pdf-review">
              <Label htmlFor="profilePdfReviewText">
                {formatMessage("profilePdf.review.heading")}
              </Label>
              <Textarea
                id="profilePdfReviewText"
                readOnly
                aria-label={formatMessage("profilePdf.review.a11y")}
                value={review.text}
                rows={8}
                className="font-mono text-[13px]"
              />
              {review.truncated ? (
                <p className="text-[13px] text-muted-foreground">
                  {formatMessage("profilePdf.review.truncated")}
                </p>
              ) : null}
            </div>
          ) : null}
        </div>

        <Separator />

        <div className="grid gap-3 p-4">
          <div className="grid gap-0.5">
            <h3 className="text-base font-semibold">{formatMessage("profilePdf.step.create")}</h3>
            <p className="text-[13px] text-muted-foreground">
              {formatMessage("profilePdf.step.create.help")}
            </p>
          </div>
          <Field
            label={formatMessage("profilePdf.label.existingProfileId")}
            hint={formatMessage("profilePdf.hint")}
            id="profilePdfExistingProfileId"
          >
            {(control) => (
              <ProfilePicker
                id={control.id}
                describedBy={control["aria-describedby"]}
                value={existingProfileId}
                onChange={setExistingProfileId}
                disabled={!editable}
              />
            )}
          </Field>
          <div className="flex items-center gap-2">
            <input
              id="profilePdfOverwrite"
              type="checkbox"
              className="size-4 accent-primary"
              checked={overwrite}
              disabled={!editable}
              aria-label={formatMessage("profilePdf.overwrite.a11y")}
              onChange={(event) => setOverwrite(event.target.checked)}
            />
            <Label htmlFor="profilePdfOverwrite" className="font-normal">
              {formatMessage("profilePdf.label.overwrite")}
            </Label>
          </div>
          <Field
            label={formatMessage("profilePdf.label.expectedRevision")}
            id="profilePdfExpectedRevision"
          >
            {(control) => (
              <Input
                {...control}
                inputMode="numeric"
                className="max-w-48"
                value={expectedRevision}
                maxLength={19}
                disabled={!editable}
                onChange={(event) => setExpectedRevision(event.target.value)}
              />
            )}
          </Field>
          <Field
            label={formatMessage("profilePdf.label.maxCharacters")}
            id="profilePdfMaxCharacters"
          >
            {(control) => (
              <Input
                {...control}
                inputMode="numeric"
                className="max-w-48"
                value={maxCharacters}
                disabled={!editable}
                onChange={(event) => setMaxCharacters(event.target.value)}
              />
            )}
          </Field>
          <div className="flex flex-wrap items-center gap-3">
            <Button
              type="button"
              id="ingestProfilePdfButton"
              disabled={!canIngest}
              onClick={() => void ingest()}
            >
              {formatMessage(
                overwrite ? "profilePdf.button.ingestReplace" : "profilePdf.button.ingest",
              )}
            </Button>
          </div>
        </div>
      </div>

      {busy !== null ? (
        <p
          role="status"
          aria-label={formatMessage("profilePdf.busy")}
          className="flex items-center gap-2 text-[13px]"
        >
          <Spinner />
          {message !== null ? formatMessage(message.id, message.params) : null}
        </p>
      ) : message !== null ? (
        <StatusMessage tone={toneOf(message.id)} className="text-[13px]">
          {formatMessage(message.id, message.params)}
        </StatusMessage>
      ) : null}

      {result !== null ? (
        <div className="grid gap-2 rounded-panel border border-border bg-card p-4">
          <h3 className="text-base font-semibold">{formatMessage("profilePdf.result.heading")}</h3>
          <ul
            className="grid gap-1 text-[13px] [overflow-wrap:anywhere] [&>li:first-child]:text-sm [&>li:first-child]:font-semibold"
            data-testid="profile-pdf-result"
          >
            {resultLines(result)
              .filter((line) => !DETAIL_IDS.has(line.id))
              .map((line) => (
                <li key={line.id + JSON.stringify(line.params ?? {})}>
                  {formatMessage(line.id, line.params)}
                </li>
              ))}
          </ul>
          <details className="text-[13px]" data-testid="profile-pdf-result-details">
            <summary className="cursor-pointer text-muted-foreground">
              {formatMessage("profilePdf.details")}
            </summary>
            <ul className="mt-1 grid gap-1 font-mono text-xs [overflow-wrap:anywhere]">
              {resultLines(result)
                .filter((line) => DETAIL_IDS.has(line.id))
                .map((line) => (
                  <li key={line.id}>{formatMessage(line.id, line.params)}</li>
                ))}
            </ul>
          </details>
        </div>
      ) : null}

      <div className="grid gap-3 rounded-panel border border-border bg-card p-4">
        <h3 className="text-base font-semibold">{formatMessage("profilePdf.lookup.heading")}</h3>
        <div className="flex flex-wrap items-end gap-3">
          <Field
            label={formatMessage("profilePdf.label.lookupProfileId")}
            id="profilePdfSourceProfileId"
            className="min-w-56 flex-1"
          >
            {(control) => (
              <ProfilePicker
                id={control.id}
                label={formatMessage("profilePdf.lookup.label")}
                value={lookupId}
                onChange={setLookupId}
                disabled={!editable}
              />
            )}
          </Field>
          <Button
            type="button"
            id="lookupProfilePdfSourceButton"
            variant="outline"
            disabled={!canLookup}
            onClick={() => void lookup()}
          >
            {formatMessage("profilePdf.button.lookup")}
          </Button>
        </div>
        {source !== null ? (
          <ul
            className="grid gap-1 border-t border-border pt-3 text-[13px] [overflow-wrap:anywhere]"
            data-testid="profile-pdf-source"
          >
            {sourceLines(source).map((line) => (
              <li key={line}>{line}</li>
            ))}
            <li>
              <details>
                <summary className="cursor-pointer text-muted-foreground">
                  {formatMessage("profilePdf.details")}
                </summary>
                <ul className="mt-1 grid gap-1 font-mono text-xs">
                  {sourceIdLines(source).map((line) => (
                    <li key={line}>{line}</li>
                  ))}
                </ul>
              </details>
            </li>
          </ul>
        ) : null}
      </div>

      <AlertDialog
        open={pending !== null}
        onOpenChange={(open) => {
          // Escape counts as Cancel, and the dialog closes only with a decision.
          if (!open && busy === null) {
            void cancel();
          }
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{formatMessage("profilePdf.dialog.title")}</AlertDialogTitle>
            <AlertDialogDescription
              title={
                pending !== null
                  ? `${pending.existingProfileId} / ${pending.documentId}`
                  : undefined
              }
            >
              {pending !== null
                ? formatMessage("profilePdf.dialog.text", {
                    expectedRevision: pending.expectedRevision,
                  })
                : null}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <Button
              type="button"
              variant="outline"
              disabled={busy !== null}
              onClick={() => void cancel()}
            >
              {formatMessage("profilePdf.button.cancel")}
            </Button>
            <Button type="button" disabled={busy !== null} onClick={() => void confirm()}>
              {formatMessage("profilePdf.button.ok")}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  );
}
