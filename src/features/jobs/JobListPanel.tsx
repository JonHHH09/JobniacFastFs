import { BriefcaseBusiness, ChevronRight, CircleAlert, RefreshCw } from "lucide-react";
import { memo } from "react";
import { formatMessage } from "../../i18n/messages";
import type { JobSummaryDto } from "../../ipc/bindings";
import { Button } from "../../ui/button";
import { EmptyState } from "../../ui/empty-state";
import { Skeleton } from "../../ui/skeleton";
import { Spinner } from "../../ui/spinner";
import { LiveStatus } from "./LiveStatus";
import { MetaParts } from "./MetaParts";
import { messageText } from "./useJobWorkspace";
import type { JobSnapshot, JobWorkspace } from "./workspace";

/**
 * One saved job row. Memoised on its own props (the summary, primitives and the stable store), so
 * a keystroke elsewhere on the page does not re-render every loaded row.
 */
const JobRow = memo(function JobRow({
  job,
  selected,
  disabled,
  workspace,
}: {
  job: JobSummaryDto;
  selected: boolean;
  disabled: boolean;
  workspace: JobWorkspace;
}) {
  return (
    <li className="min-w-0">
      <button
        type="button"
        data-slot="list-row"
        data-testid="openJobButton"
        aria-label={formatMessage("ui.job.list.openAria", { title: job.title })}
        aria-current={selected ? "true" : undefined}
        disabled={disabled}
        onClick={() => workspace.openJob(job.id)}
        className="relative flex min-h-14 w-full min-w-0 cursor-pointer items-center gap-3 px-4 py-2 text-left outline-none transition-colors before:absolute before:inset-y-0 before:left-0 before:w-[3px] before:bg-marker before:opacity-0 hover:bg-muted focus-visible:bg-muted focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:ring-inset disabled:cursor-default disabled:opacity-60 aria-[current=true]:bg-marker/20 aria-[current=true]:before:opacity-100"
      >
        <span className="grid min-w-0 flex-1 gap-0.5">
          {/* A real <h3> is not valid phrasing content inside the row button. */}
          {/* biome-ignore lint/a11y/useSemanticElements: heading inside a button */}
          <span
            role="heading"
            aria-level={3}
            data-testid="jobListTitleLabel"
            className="text-sm leading-snug font-semibold [overflow-wrap:anywhere]"
          >
            {job.title}
          </span>
          <MetaParts
            parts={[job.company, job.location]}
            className="text-[13px] leading-snug text-muted-foreground"
          />
        </span>
        <ChevronRight
          className="size-4 shrink-0 text-muted-foreground"
          strokeWidth={1.75}
          aria-hidden
        />
      </button>
    </li>
  );
});

/**
 * The saved jobs, or the search results while a search query is active.
 * Refresh and Open stay enabled while a search runs, so their requests queue behind it.
 */
export function JobListPanel({
  snapshot,
  workspace,
}: {
  snapshot: JobSnapshot;
  workspace: JobWorkspace;
}) {
  const otherBusy = (snapshot.busy !== null && snapshot.busy !== "search") || snapshot.unconfigured;
  const hasSearch = snapshot.searchQuery !== "";
  const heading = formatMessage(hasSearch ? "ui.job.heading.results" : "ui.job.heading.saved");
  // Not after a failed load (the list is unknown, not empty) and not beside the "no jobs found"
  // status of a successful empty load: one empty-state sentence at a time.
  const showEmpty =
    snapshot.jobs.length === 0 &&
    snapshot.busy === null &&
    !snapshot.listFailed &&
    snapshot.listMessage?.id !== "job.list.empty";
  const loadingEmpty =
    snapshot.jobs.length === 0 && (snapshot.busy === "list" || snapshot.busy === "search");
  // A successful empty load is an empty state (icon, sentence), not a loose status line.
  const loadedEmpty = snapshot.listMessage?.id === "job.list.empty" && snapshot.jobs.length === 0;
  const listStatus = loadedEmpty ? "" : messageText(snapshot.listMessage);
  const currentId = snapshot.current?.job.id ?? null;

  return (
    <section
      aria-labelledby="job-list-heading"
      data-testid="jobListPanel"
      className="grid min-w-0 gap-3 rounded-panel border border-border bg-card py-3"
    >
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-2 px-4">
        <h2 id="job-list-heading" className="text-base leading-snug font-semibold">
          {heading}
        </h2>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          data-testid="refreshJobsButton"
          disabled={otherBusy}
          onClick={() => workspace.refresh()}
        >
          {snapshot.busy === "list" ? (
            <Spinner />
          ) : (
            <RefreshCw className="size-4" strokeWidth={1.75} aria-hidden />
          )}
          {snapshot.busy === "list"
            ? formatMessage("ui.job.list.refreshing")
            : formatMessage("ui.job.list.refresh")}
        </Button>
      </div>
      <LiveStatus
        text={listStatus}
        data-testid="jobListStatusLabel"
        icon={
          snapshot.listFailed ? (
            <CircleAlert className="mt-0.5 size-4 shrink-0" strokeWidth={1.75} aria-hidden />
          ) : undefined
        }
        className={
          snapshot.listFailed
            ? "flex items-start gap-2 px-4 text-destructive"
            : "px-4 text-[13px] text-muted-foreground"
        }
      />
      {snapshot.listFailed ? (
        <div className="px-4">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={otherBusy}
            onClick={() => workspace.refresh()}
          >
            {formatMessage("picker.retry")}
          </Button>
        </div>
      ) : null}
      {loadedEmpty ? (
        <EmptyState
          icon={BriefcaseBusiness}
          data-testid="jobListEmptyState"
          className="mx-4"
          title={messageText(snapshot.listMessage)}
        />
      ) : null}
      {showEmpty ? (
        <EmptyState
          icon={BriefcaseBusiness}
          data-testid="jobEmptyLabel"
          className="mx-4"
          title={formatMessage(hasSearch ? "jobs.emptySearchLabel" : "jobs.emptyLabel")}
        />
      ) : null}
      {loadingEmpty ? (
        <div data-testid="jobListSkeleton" className="grid min-w-0 gap-2 px-4">
          <Skeleton className="h-12" />
          <Skeleton className="h-12" />
          <Skeleton className="h-12" />
        </div>
      ) : null}
      {snapshot.jobs.length > 0 ? (
        <ul aria-label={heading} className="min-w-0 divide-y divide-border border-y border-border">
          {snapshot.jobs.map((job) => (
            <JobRow
              key={job.id}
              job={job}
              selected={job.id === currentId}
              disabled={otherBusy}
              workspace={workspace}
            />
          ))}
        </ul>
      ) : null}
      {snapshot.nextCursor !== null ? (
        <div className="px-4">
          <Button
            type="button"
            variant="outline"
            data-testid="nextJobsPageButton"
            disabled={snapshot.busy !== null || snapshot.unconfigured}
            onClick={() => workspace.loadNextPage()}
          >
            {snapshot.busy === "listMore"
              ? formatMessage("job.progress.listMore")
              : formatMessage("ui.job.list.next")}
          </Button>
        </div>
      ) : null}
    </section>
  );
}
