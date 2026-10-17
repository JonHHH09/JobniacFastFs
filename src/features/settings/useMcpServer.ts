import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { type CommandError, commands, type SettingsDto } from "../../ipc/bindings";
import { runCommand } from "../../ipc/commands";

export const settingsQueryKey = ["settings", "get"] as const;

/** How often the status is re-read while the server is starting. */
const STARTING_POLL_MS = 1000;
/** How often it is re-read while running, so a server that died shows up (Rust refreshes then). */
export const RUNNING_POLL_MS = 5000;

/**
 * The settings and the MCP server status, plus the three actions. Every setter answers with the
 * full settings, which replace the cached value: the Rust answer is the truth, the page derives
 * nothing from its own input.
 */
export function useMcpServer() {
  const client = useQueryClient();
  const query = useQuery<SettingsDto, CommandError>({
    queryKey: settingsQueryKey,
    queryFn: () => runCommand(() => commands.settingsGet()),
    retry: false,
    refetchInterval: (state) => {
      const status = state.state.data?.mcp.status;
      if (status === "starting") {
        return STARTING_POLL_MS;
      }
      return status === "running" ? RUNNING_POLL_MS : false;
    },
  });

  const store = (settings: SettingsDto) => client.setQueryData(settingsQueryKey, settings);

  const setEnabled = useMutation<SettingsDto, CommandError, boolean>({
    mutationFn: (enabled) => runCommand(() => commands.mcpServerSetEnabled(enabled)),
    onSuccess: store,
  });
  const setPort = useMutation<SettingsDto, CommandError, number>({
    mutationFn: (port) => runCommand(() => commands.mcpServerSetPort(port)),
    onSuccess: store,
  });
  const copyConnection = useMutation<null, CommandError, void>({
    mutationFn: () => runCommand(() => commands.mcpServerCopyConnection()),
    // A failed copy may mean the server is gone: re-read the status.
    onError: () => void client.invalidateQueries({ queryKey: settingsQueryKey }),
  });

  return { query, setEnabled, setPort, copyConnection };
}
