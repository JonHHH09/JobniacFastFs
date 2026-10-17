import {
  commands,
  type DisagreementPageDto,
  type DisagreementViewDto,
  type ReviewPageDto,
  type ReviewSubmitResultDto,
  type ReviewViewDto,
} from "../../ipc/bindings";
import { requestText, runCommand } from "../../ipc/commands";
import type { ReviewSubmitRequest } from "./reviewDraft";

/** Stores the advisory review of a report and, when it diverges, its disagreement. */
export function submitReview(request: ReviewSubmitRequest): Promise<ReviewSubmitResultDto> {
  return runCommand(() => commands.reviewSubmit(requestText(request)));
}

/**
 * Reports whether the review draft holds unsaved edits, so the Rust window close gate can refuse
 * while they are not saved. Not guarded by single flight; it touches no rows.
 */
export function setDraftDirty(dirty: boolean): Promise<null> {
  return runCommand(() => commands.reviewSetDraftDirty(dirty));
}

/** One saved review by its identifier. */
export function fetchReview(id: string): Promise<ReviewViewDto> {
  return runCommand(() => commands.reviewGet(requestText({ id })));
}

/** One page of saved reviews; `reportId` null means no filter, `cursor` null the first page. */
export function fetchReviews(
  reportId: string | null,
  cursor: string | null,
): Promise<ReviewPageDto> {
  return runCommand(() => commands.reviewList(requestText({ reportId, cursor })));
}

/** One page of disagreements; `reportId` null means no filter, `cursor` null the first page. */
export function fetchDisagreements(
  reportId: string | null,
  cursor: string | null,
): Promise<DisagreementPageDto> {
  return runCommand(() => commands.reviewListDisagreements(requestText({ reportId, cursor })));
}

/** Marks a disagreement as acknowledged locally. A null reference keeps the stored one. */
export function acknowledgeDisagreement(
  disagreementId: string,
  issueReference: string | null,
): Promise<DisagreementViewDto> {
  return runCommand(() =>
    commands.reviewAcknowledgeDisagreement(requestText({ disagreementId, issueReference })),
  );
}

/** Records an existing issue reference on a disagreement. The reference is never looked up. */
export function linkDisagreement(
  disagreementId: string,
  issueReference: string,
): Promise<DisagreementViewDto> {
  return runCommand(() =>
    commands.reviewLinkDisagreement(requestText({ disagreementId, issueReference })),
  );
}
