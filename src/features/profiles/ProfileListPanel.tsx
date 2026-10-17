import { RefreshCw, UserRound } from "lucide-react";
import { formatMessage, type MessageId } from "../../i18n/messages";
import { Button } from "../../ui/button";
import { EmptyState } from "../../ui/empty-state";
import { ListRow } from "../../ui/list-row";
import { Skeleton } from "../../ui/skeleton";
import { StatusMessage } from "../../ui/status-message";
import { ProfileSection } from "./ProfileSection";
import type { ProfileListView } from "./queries";

/** The status copy of each list state; `ready` shows the rows and no status label. */
const statusMessage: Record<ProfileListView["status"], MessageId | null> = {
  idle: "profiles.idle",
  loading: "profiles.loading",
  empty: "profiles.empty",
  failed: "profiles.error",
  unconfigured: "profiles.unconfigured",
  ready: null,
};

/**
 * Read-only list of profiles with refresh and next page. The row of
 * the profile open in the editor carries the selection marker.
 */
export function ProfileListPanel({
  view,
  openProfileId,
  busy,
  onOpen,
}: {
  view: ProfileListView;
  openProfileId: string | null;
  busy: boolean;
  onOpen: (profileId: string) => void;
}) {
  const statusId = statusMessage[view.status];
  return (
    <ProfileSection
      id="profile-list"
      title={formatMessage("ui.section.profiles")}
      testId="profileListPanel"
      bare
      actions={
        <Button
          type="button"
          variant="ghost"
          size="sm"
          data-testid="profileRefreshButton"
          disabled={view.refreshing || view.loadingNext || view.status === "unconfigured"}
          onClick={() => void view.refresh()}
        >
          <RefreshCw strokeWidth={1.75} aria-hidden />
          {view.refreshing
            ? formatMessage("profiles.refreshing")
            : formatMessage("profiles.refresh")}
        </Button>
      }
    >
      {statusId && view.status === "empty" ? (
        <EmptyState
          icon={UserRound}
          role="status"
          data-testid="profileStatusLabel"
          title={formatMessage(statusId)}
        />
      ) : statusId ? (
        <StatusMessage
          tone={view.status === "failed" ? "error" : "neutral"}
          data-testid="profileStatusLabel"
        >
          {formatMessage(statusId)}
        </StatusMessage>
      ) : null}
      {view.status === "loading" && view.rows.length === 0 ? (
        <div aria-hidden className="grid gap-2">
          <Skeleton className="h-12" />
          <Skeleton className="h-12" />
          <Skeleton className="h-12" />
        </div>
      ) : null}
      {view.rows.length > 0 ? (
        <ul
          aria-label={formatMessage("ui.section.profiles")}
          className="-mx-4 divide-y divide-border border-t border-border"
        >
          {view.rows.map((row) => (
            <li key={row.id} className="min-w-0">
              <ListRow
                selected={row.id === openProfileId}
                onSelect={() => {
                  // Stays a button while busy (focus is kept); the click is ignored until idle.
                  if (!busy) {
                    onOpen(row.id);
                  }
                }}
                title={<span data-testid="profileNameLabel">{row.fullName}</span>}
                meta={<span data-testid="profileEmailLabel">{row.email}</span>}
              />
            </li>
          ))}
        </ul>
      ) : null}
      {view.hasNextPage ? (
        <div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            data-testid="nextProfilePageButton"
            disabled={view.loadingNext || view.refreshing}
            onClick={() => void view.loadNext()}
          >
            {formatMessage("profiles.next")}
          </Button>
        </div>
      ) : null}
    </ProfileSection>
  );
}
