import { CircleAlert, History as HistoryIcon } from "lucide-react";
import { Fragment, type ReactNode, useRef, useState } from "react";
import { formatMessage, isMessageId, type MessageId } from "../../i18n/messages";
import type {
  MatchBatchPageDto,
  MatchHistoryPageDto,
  MatchReportViewDto,
} from "../../ipc/bindings";
import { isCommandError, unknownFailure } from "../../ipc/commands";
import { formatInstant, humanizeEnum } from "../../lib/format";
import { cn } from "../../lib/utils";
import { Badge } from "../../ui/badge";
import { Button } from "../../ui/button";
import { EmptyState } from "../../ui/empty-state";
import { Field } from "../../ui/field";
import { ListRow } from "../../ui/list-row";
import { Marker } from "../../ui/marker";
import { ScoreMeter } from "../../ui/score-meter";
import { StatusMessage } from "../../ui/status-message";
import { JobPicker, ProfilePicker } from "../pickers";
import { ProfileSection } from "../profiles/ProfileSection";
import * as api from "./api";
import { useMatchCapabilities } from "./capabilities";
import { useJobName, useProfileName } from "./names";
import { useOpenedReport } from "./openedReport";
import { keyedRows } from "./rows";

/** The operation that holds the matching workspace. Only one runs at a time (the Rust guard too). */
type Busy = "pair" | "page" | "history" | "report" | null;

/** A catalogue message with its placeholder values. */
type Message = { id: MessageId; params?: Record<string, string | number> };

/** The last batch page and the profile it was loaded for (the next page keeps that profile). */
type Batch = { profileId: string; page: MatchBatchPageDto };

/** The last history page and the filters it was loaded with (the next page keeps them). */
type History = { profileId: string | null; jobId: string | null; page: MatchHistoryPageDto };

/**
 * Maps a failed command to its copy. Only `not_found` has its own copy.
 * A busy refusal has no copy of its own: the caller keeps the message it shows.
 */
function failureMessage(messageId: string): MessageId {
  if (messageId === "matching.unconfigured") {
    return "matching.unconfigured";
  }
  if (messageId === "matching.error.notFound") {
    return "matching.error.notFound";
  }
  return "matching.error.generic";
}

/** The Rust single-flight refusal of this workspace. */
const MATCH_BUSY = "matching.busy";

/** The badge look of a component or evidence status. Colour only supports the label. */
function statusVariant(status: string): "success" | "warning" | "destructive" | "neutral" {
  if (status === "MATCH") {
    return "success";
  }
  if (status === "PARTIAL") {
    return "warning";
  }
  if (status === "MISMATCH") {
    return "destructive";
  }
  return "neutral";
}

/**
 * The Matching screen: compare one pair, compare one jobs page, load the
 * report history and open a report. Each action handles one page; nothing is traversed automatically.
 * A failed action keeps the panels that are already shown (atomic publish).
 */
export function MatchingPage() {
  const opened = useOpenedReport();
  const [profileId, setProfileId] = useState("");
  const [jobId, setJobId] = useState("");
  const [busy, setBusy] = useState<Busy>(null);
  // Known at startup: null until the check answers. The query
  // is shared with the Reviews screen, so Rust is asked once.
  const capabilities = useMatchCapabilities();
  // A command that answered "unconfigured" switches the screen off until it is opened again.
  const [refused, setRefused] = useState(false);
  // The check itself failed (an IPC error): not the same as an explicit "not configured" answer.
  const checkFailed = capabilities.failed;
  const configured: boolean | null =
    refused || checkFailed ? false : (capabilities.data?.matching ?? null);
  const reportRef = useRef<HTMLDivElement>(null);
  const [message, setMessage] = useState<Message | null>(null);
  // The history panel has its own status slot: its progress and failures are not about a comparison.
  const [historyMessage, setHistoryMessage] = useState<Message | null>(null);
  const lastHistoryFilters = useRef<{
    profileId: string | null;
    jobId: string | null;
    cursor: string | null;
  }>({ profileId: null, jobId: null, cursor: null });
  const [report, setReport] = useState<MatchReportViewDto | null>(null);
  const [batch, setBatch] = useState<Batch | null>(null);
  const [history, setHistory] = useState<History | null>(null);
  // A ref, not only the state, so that two clicks before the next render cannot both start a request.
  const inFlight = useRef(false);

  const pairReady = profileId !== "" && jobId !== "";
  const unconfigured = configured === false;
  const locked = configured !== true || busy !== null;

  /** Runs one command under the single-flight guard. The result is published only on success. */
  async function run<T>(
    kind: Exclude<Busy, null>,
    progress: MessageId,
    task: () => Promise<T>,
    publish: (value: T) => void,
  ) {
    if (inFlight.current || configured !== true) {
      return;
    }
    inFlight.current = true;
    const forHistory = kind === "history";
    const setSlot = forHistory ? setHistoryMessage : setMessage;
    const shown = forHistory ? historyMessage : message;
    setBusy(kind);
    setSlot({ id: progress });
    try {
      publish(await task());
    } catch (error) {
      const failure = isCommandError(error) ? error : unknownFailure;
      if (failure.messageId === MATCH_BUSY) {
        // Another request holds the workspace: the message that was shown before stays.
        setSlot(shown);
        return;
      }
      if (failure.messageId === "matching.unconfigured") {
        setRefused(true);
      }
      setSlot({
        id:
          forHistory && failure.messageId !== "matching.unconfigured"
            ? "matching.history.error"
            : failureMessage(failure.messageId),
      });
    } finally {
      inFlight.current = false;
      setBusy(null);
    }
  }

  function openReport(id: string) {
    void run(
      "report",
      "matching.progress.report",
      () => api.getReport(id),
      (view) => {
        setReport(view);
        opened.setReport(view);
        setMessage(null);
        // On narrow widths the report sits below the list: bring it into view after it renders.
        requestAnimationFrame(() => {
          const reduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
          reportRef.current?.scrollIntoView?.({
            block: "start",
            behavior: reduced ? "auto" : "smooth",
          });
        });
      },
    );
  }

  function comparePair() {
    void run(
      "pair",
      "matching.progress.pair",
      () => api.analyzePair(profileId, jobId),
      (view) => {
        setReport(view);
        opened.setReport(view);
        setMessage(null);
      },
    );
  }

  function comparePage(cursor: string | null, forProfile: string) {
    void run(
      "page",
      "matching.progress.page",
      () => api.analyzePage(forProfile, cursor),
      (page) => {
        setBatch({ profileId: forProfile, page });
        setMessage({
          id: "matching.batch.summary",
          params: { reports: page.succeeded.length, failures: page.failed.length },
        });
      },
    );
  }

  function loadHistory(filters: {
    profileId: string | null;
    jobId: string | null;
    cursor: string | null;
  }) {
    lastHistoryFilters.current = filters;
    void run(
      "history",
      "matching.progress.history",
      () => api.listReports(filters.profileId, filters.jobId, filters.cursor),
      (page) => {
        setHistory({ profileId: filters.profileId, jobId: filters.jobId, page });
        setHistoryMessage(null);
      },
    );
  }

  const statusText = checkFailed
    ? formatMessage("matching.checkFailed")
    : unconfigured
      ? formatMessage("matching.unconfigured")
      : configured === null
        ? formatMessage("matching.checking")
        : message
          ? formatMessage(message.id, message.params)
          : "";
  const isFailure = checkFailed || (message?.id.startsWith("matching.error.") ?? false);
  const filterProfile = profileId === "" ? null : profileId;
  const filterJob = jobId === "" ? null : jobId;
  const openedId = report?.report.id ?? null;

  return (
    <div className="@container grid min-w-0 gap-4">
      <ProfileSection
        id="match-workspace"
        title={formatMessage("ui.match.section")}
        testId="matchingPanel"
      >
        {unconfigured ? null : (
          <div className="grid items-start gap-3 @[560px]:grid-cols-2">
            <Field id="matching-profile-id" label={formatMessage("ui.match.profile")}>
              {(control) => (
                <ProfilePicker
                  {...control}
                  testId="matchingProfileIdInput"
                  value={profileId}
                  disabled={locked}
                  onChange={setProfileId}
                />
              )}
            </Field>
            <Field id="matching-job-id" label={formatMessage("ui.match.job")}>
              {(control) => (
                <JobPicker
                  {...control}
                  testId="matchingJobIdInput"
                  value={jobId}
                  disabled={locked}
                  onChange={setJobId}
                />
              )}
            </Field>
          </div>
        )}
        <div className="flex flex-wrap items-center gap-2">
          <Button
            type="button"
            data-testid="analyzeJobMatchButton"
            disabled={locked || !pairReady}
            onClick={comparePair}
          >
            {formatMessage("ui.match.comparePair")}
          </Button>
          <Button
            type="button"
            variant="outline"
            data-testid="analyzeJobsPageButton"
            disabled={locked || profileId === ""}
            onClick={() => comparePage(null, profileId)}
          >
            {formatMessage("ui.match.comparePage")}
          </Button>
        </div>
        {!unconfigured && !pairReady ? (
          <p className="text-[13px] text-muted-foreground" data-testid="matchingPairHint">
            {formatMessage("matching.input.pair")}
          </p>
        ) : null}
        {statusText !== "" ? (
          <p
            role={isFailure ? "alert" : "status"}
            aria-live={isFailure ? "assertive" : "polite"}
            data-testid="matchWorkspaceMessage"
            className={cn(
              "flex items-start gap-2 [overflow-wrap:anywhere]",
              isFailure ? "text-destructive" : "text-muted-foreground",
            )}
          >
            {isFailure ? (
              <CircleAlert className="mt-0.5 size-4 shrink-0" strokeWidth={1.75} aria-hidden />
            ) : null}
            <span>{statusText}</span>
          </p>
        ) : null}
      </ProfileSection>

      {batch !== null ? (
        <BatchSection heading={batch.page.succeeded.length > 0} testId="matchingBatchPanel">
          <ul className="divide-y divide-border overflow-hidden rounded-md border border-border">
            {batch.page.succeeded.map((view, position) => (
              <li key={view.report.id}>
                <BatchRow
                  view={view}
                  selected={view.report.id === openedId}
                  position={position}
                  locked={locked}
                  onOpen={() => openReport(view.report.id)}
                />
              </li>
            ))}
            {batch.page.failed.map((failure) => (
              <li key={`${failure.profileId}:${failure.jobId}`}>
                <FailedRow jobId={failure.jobId} />
              </li>
            ))}
          </ul>
          {batch.page.nextCursor !== null ? (
            <div>
              <Button
                type="button"
                variant="outline"
                data-testid="nextMatchAnalysisPageButton"
                disabled={locked}
                onClick={() => comparePage(batch.page.nextCursor, batch.profileId)}
              >
                {formatMessage("ui.match.nextPage")}
              </Button>
            </div>
          ) : null}
        </BatchSection>
      ) : null}

      <ProfileSection
        id="match-history"
        title={formatMessage("matching.history.heading")}
        testId="matchingHistoryPanel"
      >
        <div>
          <Button
            type="button"
            variant="outline"
            data-testid="loadMatchReportsButton"
            disabled={locked}
            onClick={() =>
              loadHistory({ profileId: filterProfile, jobId: filterJob, cursor: null })
            }
          >
            <HistoryIcon className="size-4" strokeWidth={1.75} aria-hidden />
            {formatMessage("ui.match.loadHistory")}
          </Button>
        </div>
        {historyMessage !== null ? (
          <StatusMessage
            tone={
              historyMessage.id === "matching.history.error" ||
              historyMessage.id.startsWith("matching.error.")
                ? "error"
                : "neutral"
            }
            data-testid="matchHistoryMessage"
          >
            {formatMessage(historyMessage.id, historyMessage.params)}
          </StatusMessage>
        ) : null}
        {historyMessage?.id === "matching.history.error" ? (
          <div>
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={locked}
              onClick={() => loadHistory(lastHistoryFilters.current)}
            >
              {formatMessage("ui.match.retry")}
            </Button>
          </div>
        ) : null}
        {history === null && historyMessage === null ? (
          <EmptyState icon={HistoryIcon} title={formatMessage("matching.history.notLoaded")} />
        ) : null}
        {history !== null && history.page.reports.length === 0 && historyMessage === null ? (
          <EmptyState
            icon={HistoryIcon}
            data-testid="matchHistoryEmpty"
            title={formatMessage("matching.history.empty")}
            text={formatMessage("matching.history.emptyHint")}
          />
        ) : null}
        {history !== null && history.page.reports.length > 0 ? (
          <ul className="divide-y divide-border overflow-hidden rounded-md border border-border">
            {history.page.reports.map((view, position) => (
              <li key={view.report.id}>
                <HistoryRow
                  view={view}
                  selected={view.report.id === openedId}
                  position={position}
                  locked={locked}
                  onOpen={() => openReport(view.report.id)}
                />
              </li>
            ))}
          </ul>
        ) : null}
        {history !== null && history.page.nextCursor !== null ? (
          <div>
            <Button
              type="button"
              variant="outline"
              data-testid="nextMatchReportsPageButton"
              disabled={locked}
              onClick={() =>
                loadHistory({
                  profileId: history.profileId,
                  jobId: history.jobId,
                  cursor: history.page.nextCursor,
                })
              }
            >
              {formatMessage("ui.match.nextHistory")}
            </Button>
          </div>
        ) : null}
      </ProfileSection>

      {report !== null ? (
        <div ref={reportRef} className="scroll-mt-4">
          <MatchReportPanel view={report} />
        </div>
      ) : null}
    </div>
  );
}

/** The score as a tabular number, or a meter on wide containers. */
function ScoreCell({ view }: { view: MatchReportViewDto }) {
  const { overallScore, confidence } = view.report;
  return (
    <>
      <span
        className="font-semibold tabular-nums @[560px]:hidden"
        role="img"
        aria-label={formatMessage("matching.report.score", { overallScore, confidence })}
      >
        {overallScore}
      </span>
      <ScoreMeter
        value={overallScore}
        label={formatMessage("matching.report.score", { overallScore, confidence })}
        className="hidden w-40 @[560px]:flex"
      />
    </>
  );
}

/** The row props that make the whole row the open button (UX wave item 3). */
function openRowProps(position: number, locked: boolean, onOpen: () => void, testId: string) {
  return {
    onSelect: onOpen,
    disabled: locked,
    "data-testid": testId,
    "aria-label": formatMessage("ui.match.openReportNamed", { index: position + 1 }),
  };
}

/** One report of a jobs page: the job title, and the score. */
function BatchRow({
  view,
  selected,
  position,
  locked,
  onOpen,
}: {
  view: MatchReportViewDto;
  selected: boolean;
  position: number;
  locked: boolean;
  onOpen: () => void;
}) {
  const job = useJobName(view.report.jobId);
  return (
    <ListRow
      selected={selected}
      title={job.label}
      meta={job.description}
      trailing={<ScoreCell view={view} />}
      {...openRowProps(position, locked, onOpen, "openBatchMatchReportButton")}
    />
  );
}

/** A job of a jobs page that could not be compared. */
function FailedRow({ jobId }: { jobId: string }) {
  const job = useJobName(jobId);
  return (
    <ListRow
      title={job.label}
      meta={
        <span className="flex items-center gap-1.5 text-destructive">
          <CircleAlert className="size-4 shrink-0" strokeWidth={1.75} aria-hidden />
          {formatMessage("matching.batch.failureText")}
        </span>
      }
    />
  );
}

/** One saved report: the date as the title, the job below, staleness as a badge, and the score. */
function HistoryRow({
  view,
  selected,
  position,
  locked,
  onOpen,
}: {
  view: MatchReportViewDto;
  selected: boolean;
  position: number;
  locked: boolean;
  onOpen: () => void;
}) {
  const job = useJobName(view.report.jobId);
  const profile = useProfileName(view.report.profileId);
  return (
    <ListRow
      selected={selected}
      title={
        <time dateTime={view.report.createdAt} className="[font-variant-numeric:normal]">
          {formatInstant(view.report.createdAt)}
        </time>
      }
      meta={
        <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <span>{profile.label}</span>
          <Sep />
          <span>{job.label}</span>
          {view.stale ? (
            <Badge variant="warning">{formatMessage("matching.history.stale")}</Badge>
          ) : null}
        </span>
      }
      trailing={<ScoreCell view={view} />}
      {...openRowProps(position, locked, onOpen, "openMatchReportButton")}
    />
  );
}

/**
 * One batch page. With at least one report it is the titled section of the page; a
 * page of failures alone has no reports to head, so it is the same list without a heading.
 */
function BatchSection({
  heading,
  testId,
  children,
}: {
  heading: boolean;
  testId: string;
  children: ReactNode;
}) {
  if (heading) {
    return (
      <ProfileSection
        id="match-batch"
        title={formatMessage("matching.batch.heading")}
        testId={testId}
      >
        {children}
      </ProfileSection>
    );
  }
  return (
    <section data-testid={testId} className="grid min-w-0 gap-3">
      {children}
    </section>
  );
}

/** The share of the available points a component earned, as a whole number from 0 to 100. */
function percentOf(earned: number, available: number): number {
  return available > 0 ? Math.round((earned / available) * 100) : 0;
}

/** A hairline between two parts of a meta line. Decorative. */
function Sep() {
  return <span aria-hidden className="h-3 w-px bg-border" />;
}

/** The opened deterministic report: staleness, score, outcome, inputs, components and evidence. */
function MatchReportPanel({ view }: { view: MatchReportViewDto }) {
  const { report } = view;
  const profile = useProfileName(report.profileId);
  const job = useJobName(report.jobId);
  // An outcome the catalogue does not know has no label.
  const outcomeKnown = isMessageId(`enum.matchOutcome.${report.outcome}`);
  return (
    <ProfileSection
      id="match-report"
      title={formatMessage("ui.match.reportSection")}
      testId="matchReportPanel"
    >
      <div className="grid gap-6 @[720px]:grid-cols-[minmax(0,2fr)_minmax(0,3fr)]">
        <div className="grid min-w-0 content-start gap-4">
          {outcomeKnown ? (
            <p data-testid="matchReportOutcomeLabel">
              <Badge variant="outline" title={report.outcome}>
                {humanizeEnum("matchOutcome", report.outcome)}
              </Badge>
            </p>
          ) : null}
          <div className="grid gap-1.5">
            <ScoreMeter
              value={report.overallScore}
              label={formatMessage("matching.report.score", {
                overallScore: report.overallScore,
                confidence: report.confidence,
              })}
              className="[&_[data-tabular]]:w-14 [&_[data-tabular]]:text-3xl [&_[role=meter]]:h-4"
            />
            <p data-testid="matchReportScoreLabel" className="text-[13px] text-muted-foreground">
              {formatMessage("matching.report.coverage", { confidence: report.confidence })}
            </p>
          </div>
          <p
            data-testid="matchReportStalenessLabel"
            className={cn(
              "text-[13px] [overflow-wrap:anywhere]",
              view.stale ? "text-warning" : "text-muted-foreground",
            )}
          >
            {formatMessage(view.stale ? "matching.report.stale" : "matching.report.current")}
          </p>
          {view.componentsTruncated ? (
            <p data-testid="matchComponentsPreviewNotice" className="text-[13px]">
              {formatMessage("matching.report.componentsTruncated", {
                shown: report.components.length,
                total: view.componentsTotalCount,
              })}
            </p>
          ) : null}
          <ul className="grid gap-3">
            {keyedRows(report.components).map(({ key, row: component }) => {
              const name = humanizeEnum("matchComponent", component.component);
              return (
                <li key={key} className="grid gap-1.5">
                  <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1">
                    <span className="text-[13px] font-medium [overflow-wrap:anywhere]">{name}</span>
                    <span className="flex items-center gap-2">
                      <span className="text-[13px] tabular-nums text-muted-foreground">
                        {formatMessage("matching.report.points", {
                          earned: component.earnedPoints,
                          available: component.availablePoints,
                        })}
                      </span>
                      <Badge variant={statusVariant(component.status)} title={component.status}>
                        {humanizeEnum("matchStatus", component.status)}
                      </Badge>
                    </span>
                  </div>
                  <ScoreMeter
                    value={percentOf(component.earnedPoints, component.availablePoints)}
                    label={name}
                    showValue={false}
                    className="[&_[role=meter]]:h-2"
                  />
                </li>
              );
            })}
          </ul>
        </div>

        <div className="grid min-w-0 content-start gap-4">
          <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1.5 text-[13px] [overflow-wrap:anywhere]">
            <InputRow
              label={formatMessage("ui.match.profile")}
              name={profile.label}
              detail={profile.description}
              saved={report.profileRevision}
            />
            <InputRow
              label={formatMessage("ui.match.job")}
              name={job.label}
              detail={job.description}
              saved={report.jobRevision}
            />
          </dl>
          <p
            data-testid="matchReportAlgorithmLabel"
            className="text-[13px] text-muted-foreground [overflow-wrap:anywhere]"
          >
            {formatMessage("matching.report.method", { algorithmVersion: report.algorithmVersion })}
          </p>
          {view.evidenceTruncated ? (
            <p data-testid="matchEvidencePreviewNotice" className="text-[13px]">
              {formatMessage("matching.report.evidenceTruncated", {
                shown: report.evidence.length,
                total: view.evidenceTotalCount,
              })}
            </p>
          ) : null}
          <ul className="grid gap-3" data-testid="matchEvidenceRepeater">
            {keyedRows(report.evidence).map(({ key, row }) => (
              <li
                key={key}
                data-testid="matchEvidenceFactLabel"
                className="grid gap-0.5 text-[13px] leading-relaxed [overflow-wrap:anywhere]"
              >
                <span className={row.status === "MATCH" ? undefined : "text-muted-foreground"}>
                  {row.status === "MATCH" ? <Marker>{row.fact}</Marker> : row.fact}
                </span>
                <span className="flex flex-wrap items-center gap-x-2 text-xs text-muted-foreground">
                  {(
                    [
                      ["component", humanizeEnum("matchComponent", row.component)],
                      ["status", humanizeEnum("matchStatus", row.status)],
                      ["source", humanizeEnum("evidenceSource", row.sourceType)],
                    ] as const
                  ).map(([slot, part], index) => (
                    <Fragment key={slot}>
                      {index > 0 ? <Sep /> : null}
                      <span>{part}</span>
                    </Fragment>
                  ))}
                </span>
              </li>
            ))}
          </ul>
        </div>
      </div>
    </ProfileSection>
  );
}

/** One input of a report: its label, its name, and when that saved version was written. */
function InputRow({
  label,
  name,
  detail,
  saved,
}: {
  label: string;
  name: string;
  detail: string | null;
  saved: string;
}) {
  return (
    <>
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="grid gap-0.5">
        <span className="font-medium">{name}</span>
        {detail ? <span className="text-muted-foreground">{detail}</span> : null}
        <span className="text-muted-foreground">
          {formatMessage("matching.report.savedVersion", { date: formatInstant(saved) })}
        </span>
      </dd>
    </>
  );
}
