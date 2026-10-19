import { useIsFetching } from "@tanstack/react-query";
import { PanelRightClose, PanelRightOpen } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useCloseDiscardRequest } from "../../app/closeRefusal";
import { useOpenRequest } from "../../app/openRequests";
import { formatMessage } from "../../i18n/messages";
import { cn } from "../../lib/utils";
import { Button } from "../../ui/button";
import { Separator } from "../../ui/separator";
import { ProfileEditorPanel } from "./ProfileEditorPanel";
import { ProfileListPanel } from "./ProfileListPanel";
import { ProfileSearchPanel } from "./ProfileSearchPanel";
import { profileKeys, useProfileList } from "./queries";
import { useProfileEditor } from "./useProfileEditor";

const WORKSPACE_ID = "profile-workspace";

/**
 * The profile workspace: the search and the saved list stacked in one column, the editor beside
 * it (below it on narrow containers). The search `Open` action loads the profile into the editor,
 * after the unsaved check. A running search shares the profile guard, so it disables
 * the editor's saves and edits as well.
 */
export function ProfilesPage() {
  const list = useProfileList();
  const searching = useIsFetching({ queryKey: profileKeys.searchAll }) > 0;
  const editor = useProfileEditor(searching);
  // Hiding only hides: the workspace stays mounted, so the draft, its errors and the unsaved-changes
  // gate are exactly as they were when it is shown again.
  const [workspaceHidden, setWorkspaceHidden] = useState(false);
  // The window close gate refused with unsaved profile edits: the same confirmation as the other
  // profile actions opens, and Discard asks Rust to close again.
  useCloseDiscardRequest(editor.requestClose);
  // Opening a profile shows its workspace, whether the request came from the list, the search or
  // the command palette. A request that arrives during a load or save is ignored: the user sees
  // the busy state.
  const openProfile = (id: string) => {
    if (!editor.busy) {
      setWorkspaceHidden(false);
      editor.requestSelect(id);
    }
  };
  useOpenRequest("profile", (request) => openProfile(request.id));
  // Stacked (narrow) layout: the editor is far below the list, so bring it into view on open.
  const editorRef = useRef<HTMLDivElement>(null);
  const openId = editor.state.profileId;
  useEffect(() => {
    const node = editorRef.current;
    if (openId !== null && node && node.getBoundingClientRect().top > window.innerHeight) {
      node.scrollIntoView?.({ block: "start" });
    }
  }, [openId]);
  const ToggleIcon = workspaceHidden ? PanelRightOpen : PanelRightClose;
  return (
    <div className="@container w-full max-w-[1200px] min-w-0">
      <div className="mb-3 flex items-center justify-end gap-3">
        {workspaceHidden && editor.dirty ? (
          <span
            data-testid="hiddenUnsavedMarker"
            className="inline-flex items-center gap-1.5 text-sm text-muted-foreground"
          >
            <span aria-hidden className="size-2 rounded-full bg-marker" />
            {formatMessage("ui.editor.unsaved")}
          </span>
        ) : null}
        <Button
          type="button"
          variant="outline"
          size="sm"
          data-testid="toggleWorkspaceButton"
          aria-expanded={!workspaceHidden}
          aria-controls={WORKSPACE_ID}
          onClick={() => setWorkspaceHidden((hidden) => !hidden)}
        >
          <ToggleIcon strokeWidth={1.75} aria-hidden />
          {formatMessage(workspaceHidden ? "ui.workspace.show" : "ui.workspace.hide")}
        </Button>
      </div>
      <div
        className={cn(
          "grid items-start gap-4",
          !workspaceHidden && "@[720px]:grid-cols-[minmax(280px,340px)_minmax(0,1fr)]",
        )}
      >
        <div className="min-w-0 rounded-panel border border-border bg-card">
          <ProfileSearchPanel busy={editor.busy} onOpen={openProfile} />
          <Separator />
          <ProfileListPanel
            view={list}
            openProfileId={editor.state.profileId}
            busy={editor.busy}
            onOpen={openProfile}
          />
        </div>
        <div id={WORKSPACE_ID} ref={editorRef} hidden={workspaceHidden} className="min-w-0">
          <ProfileEditorPanel editor={editor} />
        </div>
      </div>
    </div>
  );
}
