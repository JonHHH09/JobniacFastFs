import { useForm } from "react-hook-form";
import { formatMessage } from "../../i18n/messages";
import { Button } from "../../ui/button";
import { Field } from "../../ui/field";
import { Input } from "../../ui/input";
import { blankPersonal, type PersonalDraft } from "./draft";
import { ProfileSection } from "./ProfileSection";

export type PersonalDetailsFormProps = {
  personal: PersonalDraft;
  /** Editable only in edit mode, with no operation running. */
  canEdit: boolean;
  /** The draft differs from its baseline. */
  canSave: boolean;
  /** The new-profile hint replaces the form when the profile is not saved yet. */
  saveFirst: boolean;
  hasPhoto: boolean;
  busy: boolean;
  onChange: (field: keyof PersonalDraft, value: string) => void;
  onSave: () => void;
};

/**
 * The personal details of the loaded profile. The form is mounted per baseline (the parent keys it
 * by version), so its defaults always match the loaded values. The form has no field-level checks:
 * Rust validates the date shape and the NUL rule and the save error shows the result.
 */
export function PersonalDetailsForm({
  personal,
  canEdit,
  canSave,
  saveFirst,
  hasPhoto,
  busy,
  onChange,
  onSave,
}: PersonalDetailsFormProps) {
  // No UI checks beyond the placeholder: Rust validates the date and the nationality.
  const form = useForm<PersonalDraft>({
    defaultValues: { ...blankPersonal, ...personal },
  });

  return (
    <ProfileSection
      id="personal-details"
      title={formatMessage("ui.section.personal")}
      testId="personalDetailsSection"
      bare
    >
      {saveFirst ? (
        <p className="text-sm text-muted-foreground">{formatMessage("personal.hint.saveFirst")}</p>
      ) : null}
      {!canEdit && !saveFirst && !busy ? (
        <p className="text-sm text-muted-foreground">
          {formatMessage("personal.hint.unconfigured")}
        </p>
      ) : null}
      {hasPhoto ? (
        <p className="text-sm text-muted-foreground">{formatMessage("personal.photo")}</p>
      ) : null}
      <form
        data-testid="personalDetailsForm"
        className="grid min-w-0 gap-4 @[480px]:grid-cols-2"
        noValidate
        onSubmit={form.handleSubmit(() => onSave())}
      >
        <Field label={formatMessage("ui.personal.dateOfBirth")} id="personal-date-of-birth">
          {(control) => (
            <Input
              {...control}
              data-testid="dateOfBirthInput"
              disabled={!canEdit || busy}
              {...form.register("dateOfBirth", {
                onChange: (event: { target: { value: string } }) =>
                  onChange("dateOfBirth", event.target.value),
              })}
            />
          )}
        </Field>
        <Field label={formatMessage("ui.personal.nationality")} id="personal-nationality">
          {(control) => (
            <Input
              {...control}
              data-testid="nationalityInput"
              disabled={!canEdit || busy}
              {...form.register("nationality", {
                onChange: (event: { target: { value: string } }) =>
                  onChange("nationality", event.target.value),
              })}
            />
          )}
        </Field>
        <div className="@[480px]:col-span-2">
          <Button type="submit" data-testid="savePersonalDetailsButton" disabled={!canSave || busy}>
            {busy ? formatMessage("profile.progress.save") : formatMessage("ui.personal.save")}
          </Button>
        </div>
      </form>
    </ProfileSection>
  );
}
