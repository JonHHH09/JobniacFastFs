import { Globe, Search } from "lucide-react";
import { useRef, useState } from "react";
import { formatMessage } from "../../i18n/messages";
import { ENUM_VALUES, humanizeEnum } from "../../lib/format";
import { Badge } from "../../ui/badge";
import { Button } from "../../ui/button";
import { EmptyState } from "../../ui/empty-state";
import { Field } from "../../ui/field";
import { Input } from "../../ui/input";
import { ListRow } from "../../ui/list-row";
import { Skeleton } from "../../ui/skeleton";
import { Spinner } from "../../ui/spinner";
import { StatusMessage } from "../../ui/status-message";
import { importArbeitnowRow, loadArbeitnowNextPage, scanArbeitnow } from "./api";
import {
  type ArbeitnowForm,
  type ArbeitnowOperation,
  type ArbeitnowView,
  buildScanRequest,
  canImportRow,
  EMPTY_FORM,
  failOperation,
  finishImport,
  finishPage,
  formatPostedDate,
  INITIAL_VIEW,
  importButtonMessage,
  LOCATION_MAX_LENGTH,
  QUERY_MAX_LENGTH,
  rejectionMessageId,
  startOperation,
  statusMessage,
  tidyExcerpt,
} from "./arbeitnow";
import { useArbeitnowAvailable } from "./queries";

/** The job types offered as choices. The wire value is the `jobType` enum name. */
const JOB_TYPE_CHOICES = ENUM_VALUES.jobType.filter((type) =>
  [
    "full_time",
    "part_time",
    "contract",
    "internship",
    "working_student",
    "apprenticeship",
  ].includes(type),
);

function selectedTypes(text: string): string[] {
  return text
    .split(",")
    .map((item) => item.trim())
    .filter((item) => item !== "");
}

/**
 * The Arbeitnow scan and import screen. It is rendered only when Rust reports
 * the services composed. Rust scans, holds the candidate tokens and the cursor, and answers with
 * opaque row ids; this screen sends filters and row ids back and shows the mapped copy.
 */
/** A location of "Remote" next to the Remote badge says the same thing twice. */
function showLocation(row: { location: string; remote: boolean }): boolean {
  const location = row.location.trim();
  return location !== "" && !(row.remote && location.toLowerCase() === "remote");
}

export function ArbeitnowPage() {
  const available = useArbeitnowAvailable();
  if (!available) {
    return null;
  }
  return <ArbeitnowScan />;
}

function ArbeitnowScan() {
  const [form, setForm] = useState<ArbeitnowForm>(EMPTY_FORM);
  const [view, setView] = useState<ArbeitnowView>(INITIAL_VIEW);
  // Set synchronously, so a second request before the first one renders cannot start.
  const inFlight = useRef(false);
  const busy = view.busy !== null;
  const scanning = view.busy === "scan" || view.busy === "next";
  const message = statusMessage(view);

  /** Runs one operation under the screen lock and settles the view from its outcome. */
  async function perform<T>(
    operation: ArbeitnowOperation,
    call: () => Promise<T>,
    settle: (current: ArbeitnowView, value: T) => ArbeitnowView,
  ) {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    setView((current) => startOperation(current, operation));
    try {
      const value = await call();
      setView((current) => settle(current, value));
    } catch (error) {
      const messageId = rejectionMessageId(error);
      setView((current) => failOperation(current, operation, messageId));
    } finally {
      inFlight.current = false;
    }
  }

  function scan() {
    void perform(
      "scan",
      () => scanArbeitnow(buildScanRequest(form)),
      (current, page) => finishPage(current, "scan", page),
    );
  }

  function loadNext() {
    void perform(
      "next",
      () => loadArbeitnowNextPage(),
      (current, page) => finishPage(current, "next", page),
    );
  }

  function importRow(rowId: string) {
    void perform(
      "import",
      () => importArbeitnowRow(rowId),
      (current, result) => finishImport(current, result),
    );
  }

  const messageTone = message?.includes(".error.")
    ? "error"
    : message === "arbeitnow.result.created"
      ? "success"
      : "neutral";

  return (
    <section
      aria-label={formatMessage("arbeitnow.a11y.section")}
      data-testid="arbeitnowPage"
      className="@container grid min-w-0 gap-4"
    >
      <h2 className="sr-only">{formatMessage("arbeitnow.title")}</h2>
      <div className="grid gap-4 rounded-panel border bg-card p-4">
        <div className="grid gap-3 @[720px]:grid-cols-2">
          <Field label={formatMessage("arbeitnow.label.query")}>
            {(control) => (
              <Input
                {...control}
                data-testid="arbeitnowQuery"
                aria-label={formatMessage("arbeitnow.a11y.query")}
                maxLength={QUERY_MAX_LENGTH}
                value={form.query}
                disabled={busy}
                onChange={(event) => setForm({ ...form, query: event.target.value })}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    scan();
                  }
                }}
              />
            )}
          </Field>
          <Field label={formatMessage("arbeitnow.label.location")}>
            {(control) => (
              <Input
                {...control}
                data-testid="arbeitnowLocation"
                aria-label={formatMessage("arbeitnow.a11y.location")}
                maxLength={LOCATION_MAX_LENGTH}
                value={form.location}
                disabled={busy}
                onChange={(event) => setForm({ ...form, location: event.target.value })}
              />
            )}
          </Field>
          <Field label={formatMessage("arbeitnow.label.tags")}>
            {(control) => (
              <Input
                {...control}
                data-testid="arbeitnowTags"
                aria-label={formatMessage("arbeitnow.a11y.tags")}
                value={form.tags}
                disabled={busy}
                onChange={(event) => setForm({ ...form, tags: event.target.value })}
              />
            )}
          </Field>
        </div>
        <fieldset
          data-testid="arbeitnowJobTypes"
          disabled={busy}
          className="grid min-w-0 gap-1.5 border-0 p-0"
        >
          <legend className="mb-1.5 text-[13px] font-medium">
            {formatMessage("arbeitnow.label.jobTypes")}
          </legend>
          <div className="flex flex-wrap gap-2">
            {JOB_TYPE_CHOICES.map((type) => {
              const chosen = selectedTypes(form.jobTypes);
              const checked = chosen.includes(type);
              return (
                <label
                  key={type}
                  className="flex cursor-pointer items-center gap-1.5 rounded-md border border-border px-2.5 py-1 text-[13px] has-[:checked]:border-primary has-[:checked]:bg-muted has-[:focus-visible]:ring-3 has-[:focus-visible]:ring-ring"
                >
                  <input
                    type="checkbox"
                    className="size-3.5 accent-primary"
                    checked={checked}
                    onChange={() =>
                      setForm({
                        ...form,
                        jobTypes: (checked
                          ? chosen.filter((item) => item !== type)
                          : [...chosen, type]
                        ).join(","),
                      })
                    }
                  />
                  {humanizeEnum("jobType", type)}
                </label>
              );
            })}
          </div>
        </fieldset>
        <div className="flex flex-wrap items-center justify-between gap-x-6 gap-y-3">
          <div className="flex flex-wrap items-center gap-x-5 gap-y-2">
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                className="size-4 accent-primary"
                data-testid="arbeitnowRemoteOnly"
                checked={form.remoteOnly}
                disabled={busy}
                onChange={(event) => setForm({ ...form, remoteOnly: event.target.checked })}
              />
              {formatMessage("arbeitnow.label.remote")}
            </label>
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                className="size-4 accent-primary"
                data-testid="arbeitnowVisa"
                checked={form.visaSponsorship}
                disabled={busy}
                onChange={(event) => setForm({ ...form, visaSponsorship: event.target.checked })}
              />
              {formatMessage("arbeitnow.label.visa")}
            </label>
          </div>
          <div className="flex items-center gap-3">
            {busy ? <Spinner /> : null}
            <Button
              type="button"
              data-testid="arbeitnowScanButton"
              aria-label={formatMessage("arbeitnow.a11y.scan")}
              disabled={busy}
              onClick={scan}
            >
              <Search className="size-4" strokeWidth={1.75} aria-hidden />
              {formatMessage(scanning ? "arbeitnow.button.scanning" : "arbeitnow.button.scan")}
            </Button>
          </div>
        </div>
      </div>
      {message ? (
        <StatusMessage
          tone={messageTone}
          aria-label={formatMessage("arbeitnow.a11y.status")}
          data-testid="arbeitnowStatusLabel"
        >
          {formatMessage(message)}
        </StatusMessage>
      ) : null}
      {view.rows.length > 0 && view.notice !== "" ? (
        <p data-testid="arbeitnowNoticeLabel" className="text-[13px] text-muted-foreground">
          {view.notice}
        </p>
      ) : null}
      {view.rows.length > 0 ? (
        <ul className="divide-y overflow-hidden rounded-panel border bg-card">
          {view.rows.map((row) => (
            <li key={row.rowId}>
              <ListRow
                className="items-start py-3 @[560px]:items-center"
                title={<span data-testid="arbeitnowRowTitle">{row.title}</span>}
                meta={
                  <>
                    <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
                      {row.company !== "" ? (
                        <span className="font-medium text-foreground">{row.company}</span>
                      ) : null}
                      {row.company !== "" && showLocation(row) ? (
                        <span aria-hidden className="h-3 w-px bg-border" />
                      ) : null}
                      {showLocation(row) ? <span>{row.location}</span> : null}
                      {row.remote ? (
                        <Badge variant="outline">{formatMessage("arbeitnow.row.remote")}</Badge>
                      ) : null}
                      {row.jobTypes.map((type) => (
                        <Badge key={type} variant="outline" title={type}>
                          {humanizeEnum("jobType", type)}
                        </Badge>
                      ))}
                      {row.postedAt !== null && row.postedAt !== "" ? (
                        <time dateTime={row.postedAt} className="[font-variant-numeric:normal]">
                          {formatPostedDate(row.postedAt)}
                        </time>
                      ) : null}
                    </span>
                    {row.excerpt !== "" ? (
                      <span className="mt-1 line-clamp-2">{tidyExcerpt(row.excerpt)}</span>
                    ) : null}
                  </>
                }
                trailing={
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    data-testid="arbeitnowImportButton"
                    aria-label={formatMessage("arbeitnow.a11y.import", { title: row.title })}
                    disabled={!canImportRow(view, row)}
                    onClick={() => importRow(row.rowId)}
                  >
                    {formatMessage(importButtonMessage(row.importStatus))}
                  </Button>
                }
              />
            </li>
          ))}
        </ul>
      ) : scanning ? (
        <div className="grid overflow-hidden rounded-panel border bg-card" aria-hidden>
          <Skeleton className="m-3 h-10" />
          <Skeleton className="m-3 h-10" />
          <Skeleton className="m-3 h-10" />
        </div>
      ) : message === null ? (
        <EmptyState
          icon={Globe}
          title={formatMessage("arbeitnow.idle.title")}
          text={formatMessage("arbeitnow.idle.text")}
        />
      ) : null}
      {view.hasNextPage ? (
        <div className="flex justify-center">
          <Button
            type="button"
            variant="outline"
            data-testid="arbeitnowNextButton"
            aria-label={formatMessage("arbeitnow.a11y.more")}
            disabled={busy}
            onClick={loadNext}
          >
            {formatMessage("arbeitnow.button.more")}
          </Button>
        </div>
      ) : null}
    </section>
  );
}
