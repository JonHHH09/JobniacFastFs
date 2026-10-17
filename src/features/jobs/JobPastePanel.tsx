import { zodResolver } from "@hookform/resolvers/zod";
import { useForm } from "react-hook-form";
import { formatMessage } from "../../i18n/messages";
import { Button } from "../../ui/button";
import { Field } from "../../ui/field";
import { Spinner } from "../../ui/spinner";
import { Textarea } from "../../ui/textarea";
import { JobLinkRow } from "./JobLinkRow";
import { LiveStatus } from "./LiveStatus";
import { type PasteFormValues, pasteFormSchema } from "./schema";
import { messageText } from "./useJobWorkspace";
import type { JobSnapshot, JobWorkspace } from "./workspace";

/**
 * The posting form of the jobs screens. The Add from link row is shown only
 * when `linkAvailable` (Rust reports the link capture composed).
 */
export function JobPastePanel({
  snapshot,
  workspace,
  linkAvailable = false,
  open = true,
}: {
  snapshot: JobSnapshot;
  workspace: JobWorkspace;
  linkAvailable?: boolean;
  /** The panel stays mounted when closed (its state and tests), but is not shown. */
  open?: boolean;
}) {
  const saving = snapshot.busy === "add";
  // Save posting is refused by the controls while any job operation runs.
  const busy = snapshot.busy !== null;
  const hasText = snapshot.pasteText.trim() !== "";
  // Zod is only the UX pre-check: a blank posting is not sent. Rust validates on save.
  const form = useForm<PasteFormValues>({
    resolver: zodResolver(pasteFormSchema),
    values: { text: snapshot.pasteText },
  });

  let savedLabel = "";
  if (snapshot.saveStatus === "created_job") {
    savedLabel = formatMessage("job.saved");
  } else if (snapshot.saveStatus === "reused_existing_job") {
    savedLabel = formatMessage("job.saved.reused");
  }
  const statusLabel = saving
    ? formatMessage("job.progress.save")
    : messageText(snapshot.pasteMessage);

  return (
    <section
      aria-labelledby="job-paste-heading"
      id="job-paste-panel"
      hidden={!open}
      data-testid="jobPastePanel"
      className="grid min-w-0 gap-3 rounded-panel border border-border bg-card p-4"
    >
      <h2 id="job-paste-heading" className="text-base leading-snug font-semibold">
        {formatMessage("ui.job.section.paste")}
      </h2>
      <form
        id="job-paste-form"
        className="grid min-w-0"
        onSubmit={form.handleSubmit(() => workspace.savePaste())}
      >
        <Field label={formatMessage("ui.job.paste.label")} id="job-paste-text">
          {(control) => (
            <Textarea
              {...control}
              data-testid="jobTextInput"
              className="h-16"
              placeholder={formatMessage("ui.job.paste.placeholder")}
              value={snapshot.pasteText}
              disabled={saving || snapshot.unconfigured}
              onChange={(event) => workspace.setPasteText(event.target.value)}
            />
          )}
        </Field>
      </form>
      <div className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-3">
        <Button
          type="submit"
          form="job-paste-form"
          data-testid="addJobButton"
          disabled={busy || snapshot.unconfigured || !hasText}
        >
          {saving ? <Spinner /> : null}
          {saving ? formatMessage("ui.job.paste.saving") : formatMessage("ui.job.paste.save")}
        </Button>
        {linkAvailable ? (
          <div className="min-w-0 flex-1 basis-full @[560px]:basis-72">
            <JobLinkRow snapshot={snapshot} workspace={workspace} />
          </div>
        ) : null}
      </div>
      <LiveStatus
        text={statusLabel}
        data-testid="jobPasteStatusLabel"
        className="text-muted-foreground"
      />
      <LiveStatus text={savedLabel} data-testid="jobSaveStatusLabel" className="text-success" />
    </section>
  );
}
