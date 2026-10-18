import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import {
  type CommandError,
  commands,
  type UpdateCheckDto,
  type UpdateSettingsDto,
} from "../../ipc/bindings";
import { runCommand } from "../../ipc/commands";

export const updateSettingsQueryKey = ["updates", "settings"] as const;
const checkResultQueryKey = ["updates", "checkResult"] as const;

/** The event the Rust startup check emits when it found a newer version. */
export const UPDATE_AVAILABLE_EVENT = "update:available";

/** Where a copy installed from a package finds the new version (the app cannot replace itself). */
export const RELEASES_URL = "https://github.com/JonHHH09/JobniacFastFs/releases/latest";

/**
 * The updater state and its three actions. Rust decides everything (support, install kind, the
 * answer of a check); the page only shows it. The result of the last manual check is kept in the
 * query cache, so the banner and the Settings card agree. The startup check result arrives as an
 * event, or is read from the settings when the page opened after it.
 */
export function useUpdates() {
  const client = useQueryClient();
  const settings = useQuery<UpdateSettingsDto, CommandError>({
    queryKey: updateSettingsQueryKey,
    queryFn: () => runCommand(() => commands.updateSettingsGet()),
    retry: false,
  });
  // Holds the last manual check; never fetched (the check button writes it).
  const checkResult = useQuery<UpdateCheckDto | null>({
    queryKey: checkResultQueryKey,
    queryFn: () => null,
    initialData: null,
    staleTime: Number.POSITIVE_INFINITY,
  });

  const setAutoCheck = useMutation<UpdateSettingsDto, CommandError, boolean>({
    mutationFn: (enabled) => runCommand(() => commands.updateSetAutoCheck(enabled)),
    onSuccess: (answer) => client.setQueryData(updateSettingsQueryKey, answer),
  });
  const check = useMutation<UpdateCheckDto, CommandError, void>({
    mutationFn: () => runCommand(() => commands.checkForUpdate()),
    onSuccess: (answer) => client.setQueryData(checkResultQueryKey, answer),
  });
  // On success the app restarts and this never settles; only a failure comes back.
  const install = useMutation<null, CommandError, void>({
    mutationFn: () => runCommand(() => commands.installUpdate()),
  });

  const found = checkResult.data ?? settings.data?.pending ?? null;
  return { settings, setAutoCheck, check, install, found, checked: checkResult.data };
}

/** Re-reads the updater state when Rust announces an update. A runtime without the bridge never does. */
export function useUpdateAvailableEvent(): void {
  const client = useQueryClient();
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    listen(UPDATE_AVAILABLE_EVENT, () => {
      if (active) {
        void client.invalidateQueries({ queryKey: updateSettingsQueryKey });
      }
    }).then(
      (stop) => {
        if (active) {
          unlisten = stop;
        } else {
          stop();
        }
      },
      () => undefined,
    );
    return () => {
      active = false;
      unlisten?.();
    };
  }, [client]);
}
