import type { HealthReportDto } from "../../ipc/bindings";
import type { HandlerMap } from "./util";

export const healthHandlers: HandlerMap = {
  health_check: (): HealthReportDto => ({
    status: "UP",
    errorCategory: "NONE",
    metadata: {
      checkedAt: new Date().toISOString(),
      totalChecks: 3,
      successfulChecks: 3,
      failedChecks: 0,
    },
    generatedResumeCleanup: {
      status: "HEALTHY",
      pendingCount: "0",
      processingCount: "0",
      expiredCompletedCount: "0",
      oldestDueAgeSeconds: "0",
      repeatedFailure: false,
    },
  }),
};
