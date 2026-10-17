import { formatMessage, type MessageId } from "../../i18n/messages";
import { Button } from "../../ui/button";
import { Field } from "../../ui/field";
import { Input } from "../../ui/input";
import { Spinner } from "../../ui/spinner";
import { StickyActions } from "../../ui/sticky-actions";
import { Textarea } from "../../ui/textarea";
import { ConfirmDeleteDialog } from "../deletion/ConfirmDeleteDialog";
import { LiveStatus } from "./LiveStatus";
import { messageText } from "./useJobWorkspace";
import type { EditField, JobSnapshot, JobWorkspace } from "./workspace";

const DATE_OR_EMPTY = /^(\d{4}-\d{2}-\d{2})?$/;

/** The editor fields, in display order, with their test ids. */
const FIELDS: { field: EditField; label: MessageId; testId: string; multiline?: boolean }[] = [
  { field: "title", label: "ui.job.edit.title", testId: "jobEdittitleInput" },
  { field: "company", label: "ui.job.edit.company", testId: "jobEditcompanyInput" },
  { field: "location", label: "ui.job.edit.location", testId: "jobEditlocationInput" },
  { field: "sourceLabel", label: "ui.job.edit.sourceLabel", testId: "jobEditsourceLabelInput" },
  {
    field: "experienceRequirement",
    label: "ui.job.edit.experienceRequirement",
    testId: "jobEditexperienceRequirementInput",
  },
  {
    field: "employmentType",
    label: "ui.job.edit.employmentType",
    testId: "jobEditemploymentTypeInput",
  },
  { field: "seniority", label: "ui.job.edit.seniority", testId: "jobEditseniorityInput" },
  { field: "postedAt", label: "ui.job.edit.postedAt", testId: "jobEditpostedAtInput" },
  {
    field: "description",
    label: "ui.job.edit.description",
    testId: "jobEditDescriptionInput",
    multiline: true,
  },
];

/**
 * The editor of the loaded job. Edits reach the store only from user
 * events. Save needs a changed, loaded job with a usable revision; a stale revision keeps the edits.
 */
export function JobEditorPanel({
  snapshot,
  workspace,
  deletionAvailable,
}: {
  snapshot: JobSnapshot;
  workspace: JobWorkspace;
  /** The delete use case is composed (Rust answered); the delete control is hidden otherwise. */
  deletionAvailable: boolean;
}) {
  const busy = snapshot.busy !== null || snapshot.unconfigured;
  const dirty = workspace.hasUnsavedChanges();
  const saving = snapshot.busy === "update";
  const status = messageText(snapshot.editorMessage);

  return (
    <section
      aria-labelledby="job-editor-heading"
      data-testid="jobEditorPanel"
      className="grid min-w-0"
    >
      <div className="grid min-w-0 gap-4 p-5">
        <div className="grid gap-1">
          <h2 id="job-editor-heading" className="text-base leading-snug font-semibold">
            {formatMessage("ui.job.section.editor")}
          </h2>
          <p className="text-[13px] text-muted-foreground">{formatMessage("ui.job.edit.help")}</p>
        </div>
        <div className="grid min-w-0 gap-4 @[720px]:grid-cols-2">
          {FIELDS.map(({ field, label, testId, multiline }) => (
            <Field
              key={field}
              id={`job-edit-${field}`}
              label={formatMessage(label)}
              className={multiline ? "@[720px]:col-span-2" : undefined}
            >
              {(control) =>
                multiline ? (
                  <Textarea
                    {...control}
                    data-testid={testId}
                    className="min-h-[160px]"
                    value={snapshot.draft[field]}
                    disabled={busy}
                    onChange={(event) => workspace.setField(field, event.target.value)}
                  />
                ) : (
                  <Input
                    {...control}
                    data-testid={testId}
                    // A native date picker, only while the value is a plain date (else it would blank it).
                    type={
                      field === "postedAt" && DATE_OR_EMPTY.test(snapshot.draft[field])
                        ? "date"
                        : undefined
                    }
                    value={snapshot.draft[field]}
                    disabled={busy}
                    onChange={(event) => workspace.setField(field, event.target.value)}
                  />
                )
              }
            </Field>
          ))}
          <Field
            id="job-edit-skills"
            label={formatMessage("ui.job.edit.skills")}
            className="@[720px]:col-span-2"
          >
            {(control) => (
              <Textarea
                {...control}
                data-testid="jobEditSkillsInput"
                className="min-h-[96px]"
                value={snapshot.skillsDraft}
                disabled={busy}
                onChange={(event) => workspace.setSkills(event.target.value)}
              />
            )}
          </Field>
        </div>
        <LiveStatus text={status} data-testid="jobEditorStatusLabel" />
      </div>
      <StickyActions className="rounded-b-panel">
        <Button
          type="button"
          data-testid="saveJobChangesButton"
          disabled={!workspace.canSaveChanges() || snapshot.unconfigured}
          onClick={() => workspace.saveChanges()}
        >
          {saving ? <Spinner /> : null}
          {saving ? formatMessage("ui.job.edit.saving") : formatMessage("ui.job.edit.save")}
        </Button>
        <Button
          type="button"
          variant="outline"
          data-testid="discardJobChangesButton"
          disabled={busy || !dirty}
          onClick={() => workspace.discardChanges()}
        >
          {formatMessage("ui.job.edit.discard")}
        </Button>
        <Button
          type="button"
          variant="outline"
          data-testid="reloadJobButton"
          disabled={busy}
          onClick={() => workspace.reloadLatest()}
        >
          {formatMessage("ui.job.edit.reload")}
        </Button>
        {deletionAvailable ? (
          <Button
            type="button"
            variant="destructive"
            className="@[720px]:ml-auto"
            data-testid="deleteJobButton"
            disabled={!workspace.canDeleteJob() || snapshot.unconfigured}
            onClick={() => workspace.requestDelete()}
          >
            {formatMessage("delete.button.job")}
          </Button>
        ) : null}
      </StickyActions>
      <ConfirmDeleteDialog
        kind="job"
        confirming={snapshot.deleteTarget !== null}
        message={snapshot.deleteTarget?.message ?? ""}
        onConfirm={() => workspace.confirmDelete()}
        onCancel={() => workspace.cancelDelete()}
      />
    </section>
  );
}
