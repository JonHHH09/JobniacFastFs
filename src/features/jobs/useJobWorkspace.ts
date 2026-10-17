import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useState, useSyncExternalStore } from "react";
import { formatMessage } from "../../i18n/messages";
import { JobWorkspace, type MessageRef } from "./workspace";

/**
 * The job workspace store of this page, created once per mount over the shared query client. The
 * store, not the component, holds the state, so the navigation rules are the same wherever a
 * control is rendered. The store reports its dirty flag to the window close gate while mounted.
 */
export function useJobWorkspace() {
  const queryClient = useQueryClient();
  const [workspace] = useState(() => new JobWorkspace(queryClient));
  useEffect(() => {
    workspace.attach();
    return () => workspace.detach();
  }, [workspace]);
  const snapshot = useSyncExternalStore(workspace.subscribe, workspace.getSnapshot);
  return { snapshot, workspace };
}

/** The rendered copy of a message reference, or the empty string when there is none. */
export function messageText(message: MessageRef | null): string {
  return message ? formatMessage(message.id, message.params) : "";
}
