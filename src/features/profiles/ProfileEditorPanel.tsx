import { Info, UserRound } from "lucide-react";
import { useMemo } from "react";
import { cn } from "@/lib/utils";
import { formatMessage } from "../../i18n/messages";
import { Button } from "../../ui/button";
import { EmptyState } from "../../ui/empty-state";
import { Field } from "../../ui/field";
import { Input } from "../../ui/input";
import { Spinner } from "../../ui/spinner";
import { StickyActions } from "../../ui/sticky-actions";
import { Textarea } from "../../ui/textarea";
import { ConfirmDeleteDialog } from "../deletion/ConfirmDeleteDialog";
import { CollectionEditor, type ProjectControls } from "./CollectionEditor";
import { blankPersonal } from "./draft";
import { guideText, profileGuides } from "./guide";
import { PersonalDetailsForm } from "./PersonalDetailsForm";
import { ProfileSection } from "./ProfileSection";
import { ProfileSourceNote } from "./ProfileSourceNote";
import { collectionSections } from "./schema";
import { UnsavedChangesDialog } from "./UnsavedChangesDialog";
import { useFormFeedback } from "./useFormFeedback";
import type { ProfileEditorController } from "./useProfileEditor";
import { FIELD_IDS, type FieldErrors, validateDraft } from "./validation";

const noErrors: FieldErrors = new Map();

/**
 * The profile workspace: a header with New and Reload, the profile fields, personal details and
 * every collection, and a sticky bar with the status and Discard, Delete and Save (so the save is
 * always reachable in this long form). All state and saves come from the controller.
 */
export function ProfileEditorPanel({ editor }: { editor: ProfileEditorController }) {
  const { state, busy, dispatch } = editor;
  const message = state.message;
  const projectControls: ProjectControls = {
    onTechnologyChange: (projectIndex, technologyIndex, value) =>
      dispatch({ type: "setTechnology", projectIndex, technologyIndex, value }),
    onTechnologyAdd: (projectIndex) => dispatch({ type: "addTechnology", projectIndex }),
    onTechnologyRemove: (projectIndex, technologyIndex) =>
      dispatch({ type: "removeTechnology", projectIndex, technologyIndex }),
    canSaveProject: editor.canSaveProjectRow,
    onSaveProject: editor.saveProject,
  };
  const draft = state.draft;
  const errors = useMemo(() => (draft === null ? noErrors : validateDraft(draft)), [draft]);
  // A new baseline (another profile, a finished save) starts without errors on show.
  const { feedback, blocked, attempt } = useFormFeedback(errors, state.version);
  const statusMessage = blocked
    ? formatMessage("profile.error.validation")
    : message
      ? formatMessage(message)
      : "";

  return (
    <section
      aria-labelledby="profile-editor-heading"
      data-testid="profileEditorPanel"
      className="min-w-0 rounded-panel border border-border bg-card"
    >
      <div className="flex items-center justify-between gap-x-3 border-b border-border px-4 py-3">
        <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-3 gap-y-1">
          <h2 id="profile-editor-heading" className="text-base leading-snug font-semibold">
            {formatMessage("ui.section.editor")}
          </h2>
          {state.mode === "edit" && editor.dirty ? (
            <span
              data-testid="unsavedMarker"
              className="inline-flex items-center gap-1.5 text-sm text-muted-foreground"
            >
              <span aria-hidden className="size-2 rounded-full bg-marker" />
              {formatMessage("ui.editor.unsaved")}
            </span>
          ) : null}
        </div>
        <div className="flex shrink-0 gap-2">
          <Button
            type="button"
            data-testid="newProfileButton"
            size="sm"
            disabled={busy}
            onClick={editor.requestNew}
          >
            {formatMessage("ui.editor.new")}
          </Button>
          {state.profileId ? (
            <Button
              type="button"
              data-testid="reloadProfileButton"
              variant="ghost"
              size="sm"
              disabled={busy}
              onClick={editor.requestReload}
            >
              {formatMessage("ui.editor.reload")}
            </Button>
          ) : null}
        </div>
      </div>
      {draft === null ? (
        <div className="p-4">
          <EmptyState
            icon={UserRound}
            title={formatMessage("ui.editor.empty.title")}
            text={formatMessage("ui.editor.empty.text")}
          />
        </div>
      ) : (
        <div className="@container divide-y divide-border">
          <p
            data-testid="formIntro"
            className="flex items-start gap-2 px-4 py-3 text-[13px] text-muted-foreground"
          >
            <Info className="mt-0.5 size-4 shrink-0" strokeWidth={1.75} aria-hidden />
            <span>{formatMessage("ui.form.intro")}</span>
          </p>
          <div className="grid min-w-0 gap-4 p-4 @[480px]:grid-cols-2">
            <Field
              label={formatMessage("ui.profile.fullName")}
              id={FIELD_IDS.fullName}
              hint={guideText(profileGuides.fullName)}
              error={feedback.errorFor(FIELD_IDS.fullName)}
            >
              {(control) => (
                <Input
                  {...control}
                  data-testid="fullNameInput"
                  value={draft.fullName}
                  disabled={busy}
                  onBlur={() => feedback.touch(FIELD_IDS.fullName)}
                  onChange={(event) =>
                    dispatch({ type: "setField", field: "fullName", value: event.target.value })
                  }
                />
              )}
            </Field>
            <Field
              label={formatMessage("ui.profile.email")}
              id={FIELD_IDS.email}
              hint={guideText(profileGuides.email)}
              error={feedback.errorFor(FIELD_IDS.email)}
            >
              {(control) => (
                <Input
                  {...control}
                  data-testid="emailInput"
                  type="text"
                  value={draft.email}
                  disabled={busy}
                  onBlur={() => feedback.touch(FIELD_IDS.email)}
                  onChange={(event) =>
                    dispatch({ type: "setField", field: "email", value: event.target.value })
                  }
                />
              )}
            </Field>
            <Field
              label={formatMessage("ui.profile.summary")}
              id="profile-summary"
              hint={guideText(profileGuides.summary)}
              className="@[480px]:col-span-2"
            >
              {(control) => (
                <Textarea
                  {...control}
                  data-testid="summaryInput"
                  className="h-24"
                  placeholder={formatMessage("ui.profile.summaryPlaceholder")}
                  value={draft.summary}
                  disabled={busy}
                  onChange={(event) =>
                    dispatch({ type: "setField", field: "summary", value: event.target.value })
                  }
                />
              )}
            </Field>
          </div>
          {state.mode === "edit" ? <ProfileSourceNote profileId={state.profileId} /> : null}
          <PersonalDetailsForm
            key={state.version}
            personal={state.personal ?? blankPersonal}
            canEdit={editor.canEditPersonal}
            canSave={editor.canSavePersonal}
            saveFirst={state.mode === "new"}
            hasPhoto={state.hasPhoto}
            busy={busy}
            onChange={(field, value) => dispatch({ type: "setPersonal", field, value })}
            onSave={editor.savePersonal}
          />
          {collectionSections.map((spec) => (
            <ProfileSection
              key={spec.key}
              id={`collection-${spec.key}`}
              title={spec.label}
              testId={`${spec.key}Section`}
              bare
            >
              <CollectionEditor
                spec={spec}
                rows={draft.collections[spec.key]}
                disabled={busy}
                onFieldChange={(index, field, value) =>
                  dispatch({ type: "setRowField", collection: spec.key, index, field, value })
                }
                onAdd={() => dispatch({ type: "addRow", collection: spec.key })}
                onRemove={(index) => dispatch({ type: "removeRow", collection: spec.key, index })}
                feedback={feedback}
                projects={spec.key === "projects" ? projectControls : undefined}
              />
            </ProfileSection>
          ))}
        </div>
      )}
      <StickyActions
        className={cn(
          "rounded-b-panel",
          draft === null && "has-[p:empty]:border-t-0 has-[p:empty]:py-0",
        )}
      >
        <p
          role="status"
          aria-live="polite"
          data-testid="profileMessage"
          className="min-w-0 flex-1 basis-32 text-[13px] text-muted-foreground [overflow-wrap:anywhere] empty:hidden"
        >
          {editor.loading || editor.deleting ? <Spinner className="mr-2 align-middle" /> : null}
          {editor.deleting
            ? formatMessage("profile.progress.delete")
            : editor.loading
              ? formatMessage("profile.progress.load")
              : statusMessage}
        </p>
        {
          <div
            className={cn("ml-auto flex flex-wrap items-center gap-2", draft === null && "hidden")}
          >
            {state.mode === "edit" && editor.dirty ? (
              <Button
                type="button"
                data-testid="discardDraftButton"
                variant="outline"
                disabled={busy}
                onClick={editor.requestDiscard}
              >
                {formatMessage("ui.editor.discard")}
              </Button>
            ) : null}
            {editor.deleteVisible ? (
              <Button
                type="button"
                data-testid="deleteProfileButton"
                variant="destructive"
                disabled={!editor.canDeleteProfile}
                onClick={editor.requestDelete}
              >
                {formatMessage("delete.button.profile")}
              </Button>
            ) : null}
            <Button
              type="button"
              data-testid="saveProfileButton"
              disabled={!editor.canSaveProfile}
              onClick={() => {
                // Invalid fields are marked and focused instead of sending a save Rust would refuse.
                if (!attempt()) {
                  editor.saveProfile();
                }
              }}
            >
              {editor.saving ? formatMessage("ui.editor.saving") : formatMessage("ui.editor.save")}
            </Button>
          </div>
        }
      </StickyActions>
      <UnsavedChangesDialog
        open={state.pending !== null}
        message={state.pending?.message ?? "profile.confirm.discard"}
        onKeep={editor.cancelPending}
        onDiscard={editor.confirmPending}
      />
      <ConfirmDeleteDialog
        kind="profile"
        confirming={editor.deleteConfirming}
        message={editor.deleteMessage}
        onConfirm={editor.confirmDelete}
        onCancel={editor.cancelDelete}
      />
    </section>
  );
}
