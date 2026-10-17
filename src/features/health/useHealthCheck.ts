import { useQuery } from "@tanstack/react-query";
import { type CommandError, commands, type HealthReportDto } from "../../ipc/bindings";
import { runCommand } from "../../ipc/commands";

export const healthQueryKey = ["health", "check"] as const;

/** Runs the shared health check through the generated bindings. */
export function fetchHealthReport(): Promise<HealthReportDto> {
  return runCommand(() => commands.healthCheck());
}

/**
 * The health check, run only when asked. It is disabled at startup;
 * call `refetch` to run it.
 */
export function useHealthCheck() {
  return useQuery<HealthReportDto, CommandError>({
    queryKey: healthQueryKey,
    queryFn: fetchHealthReport,
    enabled: false,
    retry: false,
  });
}
