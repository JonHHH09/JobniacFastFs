import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  type CommandError,
  commands,
  type FeatureIdDto,
  type FeaturesDto,
} from "../../ipc/bindings";
import { runCommand } from "../../ipc/commands";
import { RUNNING_POLL_MS, settingsQueryKey } from "./useMcpServer";

export const featuresQueryKey = ["settings", "features"] as const;

/** The switch change to send: which feature and the new value. */
export type FeatureChange = { id: FeatureIdDto; enabled: boolean };

/**
 * The optional features and the action that saves one switch. The saved switch applies after a
 * restart, so the answer only moves the `enabled` flags; Rust answers with the full list, which
 * replaces the cached value. Turning off an MCP capability applies at once and restarts a running
 * MCP server, so the server status and token are read again.
 */
export function useFeatures() {
  const client = useQueryClient();
  const query = useQuery<FeaturesDto, CommandError>({
    queryKey: featuresQueryKey,
    queryFn: () => runCommand(() => commands.featuresGet()),
    retry: false,
    // While a server could not be confirmed stopped, read again: Rust notices that it has exited
    // and the warning goes away without the user doing anything.
    refetchInterval: (state) => (state.state.data?.stopIncomplete ? RUNNING_POLL_MS : false),
  });
  const setEnabled = useMutation<FeaturesDto, CommandError, FeatureChange>({
    mutationFn: ({ id, enabled }) => runCommand(() => commands.featureSetEnabled(id, enabled)),
    onSuccess: (features) => {
      client.setQueryData(featuresQueryKey, features);
      void client.invalidateQueries({ queryKey: settingsQueryKey });
    },
    // A failed change may still have changed something: a revocation saves the switch, drops the
    // grant and stops the server before it can fail (a server that could not be confirmed stopped,
    // a lost restart). Read both screens again so neither shows a stale state.
    onError: () => {
      void client.invalidateQueries({ queryKey: featuresQueryKey });
      void client.invalidateQueries({ queryKey: settingsQueryKey });
    },
  });
  return { query, setEnabled };
}
