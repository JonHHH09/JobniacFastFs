import { formatMessage } from "../../i18n/messages";
import { Button } from "../../ui/button";
import { Input } from "../../ui/input";
import { Label } from "../../ui/label";
import { LiveStatus } from "./LiveStatus";
import { messageText } from "./useJobWorkspace";
import type { JobSnapshot, JobWorkspace } from "./workspace";

/**
 * The Add from link row of the save-a-posting section.
 * It is rendered only when Rust reports the link capture composed. The link is sent once, as input,
 * and is never echoed back in a message.
 */
export function JobLinkRow({
  snapshot,
  workspace,
}: {
  snapshot: JobSnapshot;
  workspace: JobWorkspace;
}) {
  const fetching = snapshot.busy === "link";
  // Add from link is refused by the controls while any job operation runs.
  const busy = snapshot.busy !== null;
  const hasLink = snapshot.linkInput.trim() !== "";
  const statusLabel = fetching
    ? formatMessage("job.progress.link")
    : messageText(snapshot.linkMessage);

  return (
    <div className="grid min-w-0 gap-2" data-testid="jobLinkRow">
      <form
        className="flex min-w-0 flex-wrap items-center gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          workspace.saveLink();
        }}
      >
        <Label htmlFor="job-link-url" className="sr-only">
          {formatMessage("ui.job.link.label")}
        </Label>
        <Input
          id="job-link-url"
          data-testid="jobLinkUrlInput"
          className="min-w-0 flex-1"
          placeholder={formatMessage("ui.job.link.placeholder")}
          value={snapshot.linkInput}
          disabled={fetching || snapshot.unconfigured}
          onChange={(event) => workspace.setLinkInput(event.target.value)}
        />
        <Button
          type="submit"
          variant="outline"
          data-testid="addJobFromLinkButton"
          disabled={busy || snapshot.unconfigured || !hasLink}
        >
          {formatMessage("ui.job.link.submit")}
        </Button>
      </form>
      <LiveStatus
        text={statusLabel}
        data-testid="jobLinkStatusLabel"
        className="text-muted-foreground"
      />
    </div>
  );
}
