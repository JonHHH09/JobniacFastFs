import { createContext, type ReactNode, useContext, useMemo, useState } from "react";
import type { MatchReportViewDto } from "../../ipc/bindings";

/**
 * The deterministic report the user has opened on the Matching screen. The Advisory reviews screen
 * reads it to offer the component details of that report.
 */
export type OpenedReport = {
  report: MatchReportViewDto | null;
  setReport: (report: MatchReportViewDto | null) => void;
};

const NO_REPORT: OpenedReport = { report: null, setReport: () => undefined };

const OpenedReportContext = createContext<OpenedReport>(NO_REPORT);

/**
 * Holds the opened report for the shell. Without this provider the report is always absent. The
 * initial report is for tests and for a screen that opens with a report already chosen.
 */
export function OpenedReportProvider({
  children,
  initialReport = null,
}: {
  children: ReactNode;
  initialReport?: MatchReportViewDto | null;
}) {
  const [report, setReport] = useState<MatchReportViewDto | null>(initialReport);
  const value = useMemo(() => ({ report, setReport }), [report]);
  return <OpenedReportContext.Provider value={value}>{children}</OpenedReportContext.Provider>;
}

/** The opened report and the setter the Matching screen uses to publish one. */
export function useOpenedReport(): OpenedReport {
  return useContext(OpenedReportContext);
}
