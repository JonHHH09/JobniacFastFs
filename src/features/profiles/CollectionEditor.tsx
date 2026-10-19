import { Plus, Trash2 } from "lucide-react";
import { formatMessage } from "../../i18n/messages";
import { Button } from "../../ui/button";
import { Field } from "../../ui/field";
import { Input } from "../../ui/input";
import { Textarea } from "../../ui/textarea";
import type { Row } from "./draft";
import { guideText, profileGuides } from "./guide";
import type { CollectionKey, CollectionSpec } from "./schema";
import type { FormFeedback } from "./useFormFeedback";

/** What each collection's items are called in the Add button (sentence case, singular). */
const ITEM_NAME: Record<CollectionKey, string> = {
  contacts: "contact",
  links: "link",
  skills: "skill",
  languages: "language",
  education: "education entry",
  experiences: "experience entry",
  projects: "project",
};

/** Suggestions for the free-text Type fields; any other value can still be typed. */
const TYPE_SUGGESTIONS: Partial<Record<string, readonly string[]>> = {
  contactType: ["email", "phone", "github", "linkedin", "website"],
  linkType: ["github", "linkedin", "website", "portfolio"],
};

/** The text of a row field: missing and null values show as empty text. */
function textOf(row: Row, key: string): string {
  const value = row[key];
  return typeof value === "string" || typeof value === "number" ? String(value) : "";
}

/**
 * The accessible name of a row's Remove button: the visible text (so voice control still matches),
 * the 1-based row number and the row's first non-empty field, so repeated buttons differ.
 */
function removeLabel(visible: string, index: number, row: Row, spec: CollectionSpec): string {
  const first = spec.fields
    .map((field) => textOf(row, field.key).trim())
    .find((text) => text !== "");
  return first ? `${visible} ${index + 1}: ${first}` : `${visible} ${index + 1}`;
}

/** The data-testid prefix of a collection key: `addExperiencesButton`, `removeExperiences0Button`. */
function capitalize(key: CollectionKey): string {
  return key.charAt(0).toUpperCase() + key.slice(1);
}

/** The project-only callbacks, present for the projects collection only. */
export type ProjectControls = {
  onTechnologyChange: (projectIndex: number, technologyIndex: number, value: string) => void;
  onTechnologyAdd: (projectIndex: number) => void;
  onTechnologyRemove: (projectIndex: number, technologyIndex: number) => void;
  canSaveProject: (projectIndex: number) => boolean;
  onSaveProject: (projectIndex: number) => void;
};

export type CollectionEditorProps = {
  spec: CollectionSpec;
  rows: Row[] | undefined;
  disabled: boolean;
  onFieldChange: (index: number, field: string, value: string) => void;
  onAdd: () => void;
  onRemove: (index: number) => void;
  /** Shows the errors of the fields once they are due; absent means no inline errors. */
  feedback?: FormFeedback;
  projects?: ProjectControls;
};

/**
 * One collection of the profile form: its rows, the add and remove actions and, for projects, the
 * technologies of each project and the project-only save. Rows without an id are new.
 */
export function CollectionEditor({
  spec,
  rows,
  disabled,
  onFieldChange,
  onAdd,
  onRemove,
  feedback,
  projects,
}: CollectionEditorProps) {
  const name = capitalize(spec.key);
  return (
    <section
      aria-label={formatMessage("ui.collection.entries", { label: spec.label })}
      className="grid min-w-0 gap-3"
    >
      {rows === undefined ? (
        <p className="text-sm text-muted-foreground">
          {formatMessage("ui.collection.notProvided")}
        </p>
      ) : null}
      {(rows ?? []).map((row, index) => (
        // Rows have no stable key before the first save; their position is the only identity here.
        <div
          // biome-ignore lint/suspicious/noArrayIndexKey: new rows have no id until they are saved
          key={`${spec.key}-${index}`}
          className="grid min-w-0 gap-3 rounded-md border border-border p-3"
        >
          <div className="grid min-w-0 gap-3 @[480px]:grid-cols-2">
            {spec.fields.map((field) => {
              const id = `${spec.key}-${index}-${field.key}`;
              const value = textOf(row, field.key);
              const testId = `${spec.key}_${index}_${field.key}Input`;
              return (
                <Field
                  key={field.key}
                  label={field.label}
                  id={id}
                  hint={guideText(field.guide)}
                  error={feedback?.errorFor(id)}
                  className={
                    field.key === "description" ||
                    (spec.fields.length === 3 && field.key === "label")
                      ? "@[480px]:col-span-2"
                      : "content-start"
                  }
                >
                  {(control) =>
                    field.key === "description" ? (
                      <Textarea
                        {...control}
                        data-testid={testId}
                        value={value}
                        disabled={disabled}
                        onBlur={() => feedback?.touch(id)}
                        onChange={(event) => onFieldChange(index, field.key, event.target.value)}
                      />
                    ) : (
                      <>
                        <Input
                          {...control}
                          data-testid={testId}
                          value={value}
                          disabled={disabled}
                          list={TYPE_SUGGESTIONS[field.key] ? `${id}-options` : undefined}
                          placeholder={field.kind === "date" ? "YYYY-MM-DD" : undefined}
                          onBlur={() => feedback?.touch(id)}
                          onChange={(event) => onFieldChange(index, field.key, event.target.value)}
                        />
                        {TYPE_SUGGESTIONS[field.key] ? (
                          <datalist id={`${id}-options`}>
                            {TYPE_SUGGESTIONS[field.key]?.map((option) => (
                              <option key={option} value={option} />
                            ))}
                          </datalist>
                        ) : null}
                      </>
                    )
                  }
                </Field>
              );
            })}
          </div>
          {projects && spec.key === "projects" ? (
            <ProjectExtras
              index={index}
              row={row}
              disabled={disabled}
              controls={projects}
              feedback={feedback}
            />
          ) : null}
          <div className="-mt-1 -mb-1 flex justify-end">
            <Button
              type="button"
              variant="ghost"
              size="icon"
              data-testid={`remove${name}${index}Button`}
              aria-label={removeLabel(formatMessage("ui.collection.remove"), index, row, spec)}
              title={formatMessage("ui.collection.remove")}
              disabled={disabled}
              onClick={() => onRemove(index)}
            >
              <Trash2 strokeWidth={1.75} aria-hidden />
            </Button>
          </div>
        </div>
      ))}
      <div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          data-testid={`add${name}Button`}
          disabled={disabled}
          onClick={onAdd}
        >
          <Plus strokeWidth={1.75} aria-hidden />
          {formatMessage("ui.collection.add", { label: ITEM_NAME[spec.key] })}
        </Button>
      </div>
    </section>
  );
}

/** The technologies sub-list and the project-only save of one project row. */
function ProjectExtras({
  index,
  row,
  disabled,
  controls,
  feedback,
}: {
  index: number;
  row: Row;
  disabled: boolean;
  controls: ProjectControls;
  feedback?: FormFeedback;
}) {
  const technologies = Array.isArray(row.technologies) ? (row.technologies as Row[]) : [];
  const descriptionId = `project-save-description-${index}`;
  return (
    <div className="grid min-w-0 gap-3">
      <h3 className="text-sm font-semibold">{formatMessage("ui.technology.heading")}</h3>
      <div data-testid={`projectTechnologiesRepeater${index}`} className="grid min-w-0 gap-2">
        {technologies.map((technology, techIndex) => {
          const id = `project-${index}-technology-${techIndex}`;
          return (
            <div
              // biome-ignore lint/suspicious/noArrayIndexKey: technologies have no id until saved
              key={`technology-${techIndex}`}
              className="flex min-w-0 flex-wrap items-end gap-2"
            >
              <Field
                label={formatMessage("ui.technology.label")}
                id={id}
                hint={guideText(profileGuides.technology)}
                error={feedback?.errorFor(id)}
                className="min-w-0 flex-1"
              >
                {(control) => (
                  <Input
                    {...control}
                    data-testid={`projects_${index}_technology_${techIndex}_input`}
                    placeholder={formatMessage("ui.technology.label")}
                    value={textOf(technology, "technology")}
                    disabled={disabled}
                    onBlur={() => feedback?.touch(id)}
                    onChange={(event) =>
                      controls.onTechnologyChange(index, techIndex, event.target.value)
                    }
                  />
                )}
              </Field>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                data-testid={`removeProjectTechnology${index}_${techIndex}`}
                aria-label={`${formatMessage("ui.technology.remove")} ${techIndex + 1}${
                  textOf(technology, "technology").trim()
                    ? `: ${textOf(technology, "technology").trim()}`
                    : ""
                }`}
                disabled={disabled}
                onClick={() => controls.onTechnologyRemove(index, techIndex)}
              >
                {formatMessage("ui.technology.remove")}
              </Button>
            </div>
          );
        })}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="button"
          variant="outline"
          size="sm"
          data-testid={`addProjectTechnology${index}Button`}
          disabled={disabled}
          onClick={() => controls.onTechnologyAdd(index)}
        >
          {formatMessage("ui.technology.add")}
        </Button>
        <Button
          type="button"
          size="sm"
          data-testid={`saveProjectFields${index}Button`}
          aria-describedby={descriptionId}
          disabled={disabled || !controls.canSaveProject(index)}
          onClick={() => controls.onSaveProject(index)}
        >
          {formatMessage("ui.project.save")}
        </Button>
        <span id={descriptionId} className="text-sm text-muted-foreground">
          {formatMessage("ui.project.saveNote")}
        </span>
      </div>
    </div>
  );
}
