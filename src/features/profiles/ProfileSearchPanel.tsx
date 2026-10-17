import { zodResolver } from "@hookform/resolvers/zod";
import { Search } from "lucide-react";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { cn } from "@/lib/utils";
import { formatMessage, isMessageId, type MessageId } from "../../i18n/messages";
import { humanizeEnum } from "../../lib/format";
import { Button } from "../../ui/button";
import { Field } from "../../ui/field";
import { Input } from "../../ui/input";
import { ListRow } from "../../ui/list-row";
import { Spinner } from "../../ui/spinner";
import { ProfileSection } from "./ProfileSection";
import { useProfileSearch } from "./queries";
import { type SearchFormValues, searchFormSchema } from "./schemas";

/** The counts of a search response, as the summary copy needs them. */
type SearchCounts = {
  totalMatches: string | null;
  matchedCount: string;
  returnedCount: number;
  hasMore: boolean;
};

/**
 * The summary copy of a search. `totalMatches` is null when the
 * total is unknown; the counts keep the text the wire carries.
 */
function summaryFor(
  counts: SearchCounts,
  query: string,
): { id: MessageId; params: Record<string, string | number> } {
  if (counts.totalMatches === "0") {
    return { id: "profileSearch.summary.none", params: { query } };
  }
  const params = {
    returned: counts.returnedCount,
    matched: counts.matchedCount,
    total: counts.totalMatches ?? "",
  };
  if (counts.totalMatches === null) {
    return { id: "profileSearch.summary.unknownTotal", params };
  }
  if (counts.hasMore) {
    return { id: "profileSearch.summary.truncated", params };
  }
  return { id: "profileSearch.summary.full", params };
}

/**
 * Find a profile: a required query (checked here, validated in Rust), the results, and the `Open`
 * action of each result. Enter in the input submits the search.
 */
export function ProfileSearchPanel({
  busy,
  onOpen,
}: {
  busy: boolean;
  onOpen: (profileId: string) => void;
}) {
  const [submitted, setSubmitted] = useState("");
  const form = useForm<SearchFormValues>({
    resolver: zodResolver(searchFormSchema),
    defaultValues: { query: "" },
  });
  const search = useProfileSearch(submitted);
  const data = search.data;

  let statusText = "";
  const formError = form.formState.errors.query?.message;
  if (formError) {
    statusText = isMessageId(formError) ? formatMessage(formError) : formError;
  } else if (search.isError) {
    statusText = formatMessage("profileSearch.failed");
  } else if (data) {
    const summary = summaryFor(data, submitted);
    statusText = formatMessage(summary.id, summary.params);
  }

  return (
    <ProfileSection
      id="profile-search"
      title={formatMessage("ui.section.search")}
      testId="profileSearchPanel"
      bare
    >
      <form
        className="grid gap-2"
        onSubmit={form.handleSubmit((values) => {
          const query = values.query.trim();
          // The same query starts a new search (a failed one can be retried).
          if (query === submitted) {
            void search.refetch();
            return;
          }
          setSubmitted(query);
        })}
        noValidate
      >
        <Field label={formatMessage("ui.search.label")} id="profile-search-input">
          {(control) => (
            <div className="flex gap-2">
              <Input
                {...control}
                data-testid="profileSearchInput"
                className="min-w-0 flex-1"
                placeholder={formatMessage("ui.search.placeholder")}
                aria-invalid={formError ? true : undefined}
                disabled={busy}
                {...form.register("query")}
              />
              <Button
                type="submit"
                variant="outline"
                data-testid="searchProfilesButton"
                disabled={busy || search.isFetching}
              >
                {search.isFetching ? <Spinner /> : <Search strokeWidth={1.75} aria-hidden />}
                {search.isFetching
                  ? formatMessage("profileSearch.button.busy")
                  : formatMessage("profileSearch.button.idle")}
              </Button>
            </div>
          )}
        </Field>
      </form>
      <p
        role="status"
        aria-live="polite"
        data-testid="profileSearchStatusLabel"
        className={cn(
          "empty:hidden [overflow-wrap:anywhere]",
          formError || search.isError ? "text-destructive" : "text-muted-foreground",
        )}
      >
        {statusText}
      </p>
      {data && data.profiles.length > 0 ? (
        <ul
          aria-label={formatMessage("ui.search.results")}
          className="-mx-4 divide-y divide-border border-t border-border"
        >
          {data.profiles.map((match) => (
            <li key={match.profile.id} className="min-w-0">
              <ListRow
                title={<span data-testid="profileNameLabel">{match.profile.fullName}</span>}
                meta={
                  <>
                    <span className="block" data-testid="profileEmailLabel">
                      {match.profile.email}
                    </span>
                    <span className="block">
                      {formatMessage("ui.search.matched", {
                        fields: match.matchedFields
                          .map((f) => humanizeEnum("profileField", f))
                          .join(", "),
                      })}
                    </span>
                  </>
                }
                trailing={
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    data-testid="openSearchResultButton"
                    aria-label={formatMessage("ui.search.openAria", {
                      name: match.profile.fullName,
                    })}
                    disabled={busy}
                    onClick={() => onOpen(match.profile.id)}
                  >
                    {formatMessage("ui.search.open")}
                  </Button>
                }
              />
            </li>
          ))}
        </ul>
      ) : null}
    </ProfileSection>
  );
}
