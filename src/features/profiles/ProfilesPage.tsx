import { useIsFetching } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { useCloseDiscardRequest } from "../../app/closeRefusal";
import { useOpenRequest } from "../../app/openRequests";
import { Separator } from "../../ui/separator";
import { ProfileEditorPanel } from "./ProfileEditorPanel";
import { ProfileListPanel } from "./ProfileListPanel";
import { ProfileSearchPanel } from "./ProfileSearchPanel";
import { profileKeys, useProfileList } from "./queries";
import { useProfileEditor } from "./useProfileEditor";

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
  // The window close gate refused with unsaved profile edits: the same confirmation as the other
  // profile actions opens, and Discard asks Rust to close again.
  useCloseDiscardRequest(editor.requestClose);
  // The command palette asks to open a profile: the same path as the search `Open` action.
  // A request that arrives during a load or save is ignored: the user sees the busy state.
  useOpenRequest("profile", (request) => {
    if (!editor.busy) {
      editor.requestSelect(request.id);
    }
  });
  // Stacked (narrow) layout: the editor is far below the list, so bring it into view on open.
  const editorRef = useRef<HTMLDivElement>(null);
  const openId = editor.state.profileId;
  useEffect(() => {
    const node = editorRef.current;
    if (openId !== null && node && node.getBoundingClientRect().top > window.innerHeight) {
      node.scrollIntoView?.({ block: "start" });
    }
  }, [openId]);
  return (
    <div className="@container w-full max-w-[1200px] min-w-0">
      <div className="grid items-start gap-4 @[720px]:grid-cols-[minmax(280px,340px)_minmax(0,1fr)]">
        <div className="min-w-0 rounded-panel border border-border bg-card">
          <ProfileSearchPanel busy={editor.busy} onOpen={editor.requestSelect} />
          <Separator />
          <ProfileListPanel
            view={list}
            openProfileId={editor.state.profileId}
            busy={editor.busy}
            onOpen={editor.requestSelect}
          />
        </div>
        <div ref={editorRef} className="min-w-0">
          <ProfileEditorPanel editor={editor} />
        </div>
      </div>
    </div>
  );
}
