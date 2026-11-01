import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  type CommandError,
  commands,
  type FeatureIdDto,
  type FeaturesDto,
} from "../../ipc/bindings";
import { runCommand } from "../../ipc/commands";

export const featuresQueryKey = ["settings", "features"] as const;

/** The switch change to send: which feature and the new value. */
export type FeatureChange = { id: FeatureIdDto; enabled: boolean };

/**
 * The optional features and the action that saves one switch. The saved switch applies after a
 * restart, so the answer only moves the `enabled` flags; Rust answers with the full list, which
 * replaces the cached value.
 */
export function useFeatures() {
  const client = useQueryClient();
  const query = useQuery<FeaturesDto, CommandError>({
    queryKey: featuresQueryKey,
    queryFn: () => runCommand(() => commands.featuresGet()),
    retry: false,
  });
  const setEnabled = useMutation<FeaturesDto, CommandError, FeatureChange>({
    mutationFn: ({ id, enabled }) => runCommand(() => commands.featureSetEnabled(id, enabled)),
    onSuccess: (features) => client.setQueryData(featuresQueryKey, features),
  });
  return { query, setEnabled };
}
