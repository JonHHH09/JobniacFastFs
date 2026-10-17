import { BriefcaseBusiness } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useOpenRequest } from "../../app/openRequests";
import { formatMessage } from "../../i18n/messages";
import { EmptyState } from "../../ui/empty-state";
import { Separator } from "../../ui/separator";
import { useDeletionAvailable } from "../deletion/queries";
import { JobDetailPanel } from "./JobDetailPanel";
import { JobEditorPanel } from "./JobEditorPanel";
import { JobListPanel } from "./JobListPanel";
import { JobPastePanel } from "./JobPastePanel";
import { JobSearchPanel } from "./JobSearchPanel";
import { LiveStatus } from "./LiveStatus";
import { useJobLinkAvailable } from "./queries";
import { messageText, useJobWorkspace } from "./useJobWorkspace";

/**
 * The job screens: search saved jobs, save a posting, then one workspace with the saved jobs or the
 * results beside the job detail and the editor. Nothing loads at startup.
 */
export function JobsPage() {
  const { snapshot, workspace } = useJobWorkspace();
  // The capability is asked only once a job is loaded, so an empty screen makes no call.
  const deletionAvailable = useDeletionAvailable(snapshot.current !== null);
  // The link row is shown only when Rust composed the link capture.
  const linkAvailable = useJobLinkAvailable();
  useOpenRequest("job", (request) => void workspace.openJob(request.id));
  const [addOpen, setAddOpen] = useState(false);
  const detailRef = useRef<HTMLDivElement>(null);
  const currentId = snapshot.current?.job.id ?? null;
  // Stacked (narrow) layout: the detail is far below the list, so bring it into view on open.
  useEffect(() => {
    const node = detailRef.current;
    if (currentId !== null && node && node.getBoundingClientRect().top > window.innerHeight) {
      node.scrollIntoView?.({ block: "start" });
    }
  }, [currentId]);
  const status = messageText(snapshot.message);
  // The "open a job" hint only makes sense while there is a job to open.
  const showPlaceholder = snapshot.current === null && snapshot.jobs.length > 0;
  return (
    <div className="@container grid min-w-0 gap-4">
      <LiveStatus text={status} data-testid="jobStatusLabel" className="text-muted-foreground" />
      <JobSearchPanel
        snapshot={snapshot}
        workspace={workspace}
        addOpen={addOpen}
        onToggleAdd={() => setAddOpen((value) => !value)}
      />
      <JobPastePanel
        snapshot={snapshot}
        workspace={workspace}
        linkAvailable={linkAvailable}
        open={addOpen}
      />
      <div
        className={
          showPlaceholder || snapshot.current !== null
            ? "grid min-w-0 gap-4 @[720px]:grid-cols-[minmax(280px,340px)_minmax(0,1fr)] @[720px]:items-start"
            : "grid min-w-0 gap-4"
        }
      >
        <JobListPanel snapshot={snapshot} workspace={workspace} />
        {snapshot.current !== null ? (
          <div ref={detailRef} className="min-w-0 rounded-panel border border-border bg-card">
            <JobDetailPanel job={snapshot.current} />
            <Separator />
            <JobEditorPanel
              snapshot={snapshot}
              workspace={workspace}
              deletionAvailable={deletionAvailable}
            />
          </div>
        ) : showPlaceholder ? (
          <div className="hidden @[720px]:block">
            <EmptyState icon={BriefcaseBusiness} title={formatMessage("ui.job.detail.empty")} />
          </div>
        ) : null}
      </div>
    </div>
  );
}
