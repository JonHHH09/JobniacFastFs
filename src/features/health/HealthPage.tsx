import { RefreshCw } from "lucide-react";
import { copy } from "../../i18n/copy";
import type { CleanupStatusDto, HealthReportDto } from "../../ipc/bindings";
import { isCommandError } from "../../ipc/commands";
import { formatInstant, formatRelative, humanizeEnum } from "../../lib/format";
import { Button } from "../../ui/button";
import { Card } from "../../ui/card";
import { PageHeader } from "../../ui/page-header";
import { Separator } from "../../ui/separator";
import { Spinner } from "../../ui/spinner";
import { useHealthCheck } from "./useHealthCheck";

/**
 * What the page shows, read from the report the Rust side returns. The page derives nothing: the
 * status, categories and counts all come from the shared health use case.
 */
type HealthView = {
  status: "UP" | "DOWN" | null;
  errorCategory: string | null;
  checkedAt: string | null;
  successfulChecks: number;
  failedChecks: number;
  cleanupStatus: CleanupStatusDto | null;
};

const emptyView: HealthView = {
  status: null,
  errorCategory: null,
  checkedAt: null,
  successfulChecks: 0,
  failedChecks: 0,
  cleanupStatus: null,
};

/** The unknown report: a failed check shows DOWN / UNKNOWN with zero counts. */
const failedView: HealthView = {
  status: "DOWN",
  errorCategory: "UNKNOWN",
  checkedAt: null,
  successfulChecks: 0,
  failedChecks: 0,
  cleanupStatus: "UNKNOWN",
};

/** No report yet shows the not-checked state; a failed check shows the unknown report, never its text. */
function toView(report: HealthReportDto | undefined, failed: boolean): HealthView {
  if (failed) {
    return failedView;
  }
  if (!report) {
    return emptyView;
  }
  return {
    status: report.status,
    errorCategory: report.errorCategory,
    checkedAt: report.metadata.checkedAt,
    successfulChecks: report.metadata.successfulChecks,
    failedChecks: report.metadata.failedChecks,
    cleanupStatus: report.generatedResumeCleanup.status,
  };
}

function explanation(view: HealthView, checking: boolean): string {
  if (checking) {
    return copy.health.explainChecking;
  }
  if (view.status === "DOWN" && view.errorCategory && view.errorCategory !== "NONE") {
    return copy.health.explainFailed(humanizeEnum("errorCategory", view.errorCategory));
  }
  // A result is on screen once the status is UP: the "request the result" hint would contradict it.
  if (view.status === "UP") {
    return "";
  }
  return copy.health.explainIdle;
}

function cleanupText(status: CleanupStatusDto | null): string {
  if (status === null) {
    return copy.health.cleanupNotAvailable;
  }
  if (status === "UNKNOWN") {
    return copy.health.cleanupUnknown;
  }
  return copy.health.cleanupStatus(humanizeEnum("cleanupStatus", status));
}

/** The health check state shared by the toolbar and the result (one query, one check at a time). */
function useHealth() {
  const { data, error, isFetching, refetch } = useHealthCheck();
  // A busy refusal means a check already runs elsewhere: it is not a failed check.
  const failed = error !== null && !(isCommandError(error) && error.code === "busy");
  return { checking: isFetching, view: toView(data, failed && !isFetching), refetch };
}

/** The page header: the title, one line about the page and the Refresh button. */
export function HealthToolbar() {
  const { checking, refetch } = useHealth();
  return (
    <PageHeader
      role="toolbar"
      aria-label={copy.health.toolbar}
      data-testid="healthToolbar"
      title={copy.health.title}
      description={copy.health.description}
      actions={
        <Button
          type="button"
          // cancelRefetch: false keeps a second activation from cancelling the running check and
          // starting another one, which the Rust single-flight guard would refuse.
          onClick={() => void refetch({ cancelRefetch: false })}
          disabled={checking}
          aria-describedby="health-explanation"
        >
          {checking ? <Spinner /> : <RefreshCw className="size-4" strokeWidth={1.75} aria-hidden />}
          {checking ? copy.health.refreshing : copy.health.refresh}
        </Button>
      }
    />
  );
}

/** The result of the last check: one quiet panel with the status, check details, cleanup and the footer. */
export function HealthResult() {
  const { checking, view } = useHealth();
  const statusText = view.status
    ? humanizeEnum("healthStatus", view.status)
    : copy.health.statusNone;
  const statusState = view.status === "UP" ? "up" : view.status === "DOWN" ? "down" : "none";
  const statusClass =
    statusState === "up"
      ? "text-success"
      : statusState === "down"
        ? "text-destructive"
        : "text-muted-foreground";

  return (
    <section
      aria-labelledby="health-result-heading"
      className="@container grid min-w-0"
      data-testid="healthResult"
    >
      <Card className="gap-0 py-0">
        <div className="grid min-w-0 gap-1.5 px-5 py-5">
          <h2 id="health-result-heading" className="sr-only">
            {copy.health.statusHeading}
          </h2>
          {/* The one live region: its text is the full sentence, so the outcome is announced once. */}
          <p
            role="status"
            aria-live="polite"
            data-state={statusState}
            className={`text-[40px] leading-tight font-bold [overflow-wrap:anywhere] ${statusClass}`}
          >
            <span aria-hidden="true">{statusText}</span>
            <span className="sr-only">{copy.health.statusAria(statusText)}</span>
          </p>
          <p
            id="health-explanation"
            className="text-muted-foreground [overflow-wrap:anywhere] empty:hidden"
          >
            {explanation(view, checking)}
          </p>
        </div>
        <Separator />
        <div className="grid min-w-0 gap-3 px-5 py-4" data-testid="healthCheckDetails">
          <h2 id="health-details-heading" className="text-base font-semibold">
            {copy.health.checkDetails}
          </h2>
          <dl className="grid min-w-0 items-start gap-x-6 gap-y-3 @[560px]:grid-cols-3">
            <div className="grid gap-0.5">
              <dt className="text-[13px] text-muted-foreground">{copy.health.checkedAt}</dt>
              <dd className="grid gap-0.5 [overflow-wrap:anywhere]">
                {view.checkedAt === null ? (
                  copy.health.notAvailable
                ) : (
                  <>
                    <time
                      dateTime={view.checkedAt}
                      title={view.checkedAt}
                      className="[font-variant-numeric:normal]"
                    >
                      {formatInstant(view.checkedAt)}
                    </time>
                    <span className="text-[13px] text-muted-foreground">
                      {formatRelative(view.checkedAt)}
                    </span>
                  </>
                )}
              </dd>
            </div>
            <div className="grid gap-0.5">
              <dt className="text-[13px] text-muted-foreground">{copy.health.successful}</dt>
              <dd className="tabular-nums">{view.successfulChecks}</dd>
            </div>
            <div className="grid gap-0.5">
              <dt className="text-[13px] text-muted-foreground">{copy.health.failed}</dt>
              <dd className="tabular-nums">{view.failedChecks}</dd>
            </div>
          </dl>
        </div>
        <Separator />
        <div className="grid min-w-0 gap-1 px-5 py-4">
          <h2 id="cleanup-heading" className="text-base font-semibold">
            {copy.health.cleanupTitle}
          </h2>
          <p className="[overflow-wrap:anywhere]">{cleanupText(view.cleanupStatus)}</p>
          <p className="mt-2 text-[13px] text-muted-foreground [overflow-wrap:anywhere]">
            {copy.health.footer}
          </p>
        </div>
      </Card>
    </section>
  );
}

/** The whole health screen: toolbar and result together (the shell places them apart). */
export function HealthPage() {
  return (
    <>
      <HealthToolbar />
      <HealthResult />
    </>
  );
}
