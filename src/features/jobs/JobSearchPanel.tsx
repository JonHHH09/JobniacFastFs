import { zodResolver } from "@hookform/resolvers/zod";
import { Plus, Search } from "lucide-react";
import { useForm } from "react-hook-form";
import { formatMessage } from "../../i18n/messages";
import { Button } from "../../ui/button";
import { Input } from "../../ui/input";
import { Label } from "../../ui/label";
import { Toolbar } from "../../ui/toolbar";
import { LiveStatus } from "./LiveStatus";
import { type SearchFormValues, searchFormSchema } from "./schema";
import { messageText } from "./useJobWorkspace";
import type { JobSnapshot, JobWorkspace } from "./workspace";

/**
 * The search toolbar of the saved jobs. Search and Show saved jobs stay
 * enabled while a search runs: the newest request wins.
 */
export function JobSearchPanel({
  snapshot,
  workspace,
  addOpen,
  onToggleAdd,
}: {
  snapshot: JobSnapshot;
  workspace: JobWorkspace;
  addOpen: boolean;
  onToggleAdd: () => void;
}) {
  const otherBusy = (snapshot.busy !== null && snapshot.busy !== "search") || snapshot.unconfigured;
  const hasQuery = snapshot.searchInput.trim() !== "";
  // Zod is only the UX pre-check: a blank query is not sent. Rust validates the query.
  const form = useForm<SearchFormValues>({
    resolver: zodResolver(searchFormSchema),
    values: { query: snapshot.searchInput },
  });

  return (
    <section
      aria-label={formatMessage("ui.job.section.search")}
      data-testid="jobSearchPanel"
      className="grid min-w-0 gap-1.5"
    >
      <form onSubmit={form.handleSubmit(() => workspace.search())}>
        <Toolbar
          end={
            <>
              <Button
                type="submit"
                variant="outline"
                data-testid="jobSearchButton"
                disabled={otherBusy || !hasQuery}
              >
                {formatMessage("ui.job.search.submit")}
              </Button>
              {snapshot.searchQuery !== "" ? (
                <Button
                  type="button"
                  variant="ghost"
                  data-testid="clearJobSearchButton"
                  disabled={otherBusy}
                  onClick={() => workspace.showAllJobs()}
                >
                  {formatMessage("ui.job.search.showAll")}
                </Button>
              ) : null}
              <Button
                type="button"
                data-testid="addPostingToggleButton"
                aria-expanded={addOpen}
                aria-controls="job-paste-panel"
                onClick={onToggleAdd}
              >
                <Plus className="size-4" strokeWidth={1.75} aria-hidden />
                {formatMessage("ui.job.addPosting")}
              </Button>
            </>
          }
        >
          <Label htmlFor="job-search-input" className="sr-only">
            {formatMessage("ui.job.search.label")}
          </Label>
          <div className="relative min-w-0 flex-1">
            <Search
              className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
              strokeWidth={1.75}
              aria-hidden
            />
            <Input
              id="job-search-input"
              data-testid="jobSearchInput"
              className="pl-8"
              maxLength={512}
              placeholder={formatMessage("ui.job.search.placeholder")}
              value={snapshot.searchInput}
              disabled={snapshot.unconfigured}
              onChange={(event) => workspace.setSearchInput(event.target.value)}
            />
          </div>
        </Toolbar>
      </form>
      <LiveStatus
        text={messageText(snapshot.searchMessage)}
        data-testid="jobSearchStatusLabel"
        className="text-muted-foreground"
      />
    </section>
  );
}
