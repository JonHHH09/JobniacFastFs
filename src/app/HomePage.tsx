import { Suspense, useEffect, useRef, useState } from "react";
import { OpenedReportProvider } from "../features/matching/openedReport";
import { copy } from "../i18n/copy";
import { cn } from "../lib/utils";
import { PageHeader } from "../ui/page-header";
import { Skeleton } from "../ui/skeleton";
import { SECTIONS, type SectionDefinition, type SectionId } from "./sections";
import { useActiveSection } from "./useActiveSection";

/** What a section shows while its code loads. */
function SectionSkeleton() {
  return (
    <div role="status" className="grid gap-3">
      <span className="sr-only">{copy.shell.loadingSection}</span>
      <Skeleton className="h-24 w-full rounded-panel" />
      <Skeleton className="h-40 w-full rounded-panel" />
    </div>
  );
}

function SectionPanel({ section, active }: { section: SectionDefinition; active: boolean }) {
  const Page = section.component;
  return (
    <div
      id={`section-panel-${section.id}`}
      data-testid={`section-${section.id}`}
      hidden={!active}
      tabIndex={-1}
      className={cn(
        "grid gap-[18px] outline-none",
        section.width === "wide" ? "max-w-[1200px]" : "max-w-[880px]",
        "animate-section-in motion-reduce:animate-none",
        "w-full",
      )}
    >
      {section.description ? (
        <PageHeader title={section.label} description={section.description} />
      ) : null}
      <Suspense fallback={<SectionSkeleton />}>
        <Page />
      </Suspense>
    </div>
  );
}

/**
 * The content area: one panel per section. A section mounts the first time it is visited and then
 * stays mounted, hidden while another is active, so unsaved drafts survive a switch and nothing but
 * the default section loads at startup. The opened matching report is shared across the
 * Matches and Reviews sections, so one provider wraps every panel. After a change the focus moves to
 * the heading of the new section.
 */
export function HomePage() {
  const { active } = useActiveSection();
  const [visited, setVisited] = useState<ReadonlySet<SectionId>>(() => new Set([active]));
  if (!visited.has(active)) {
    setVisited(new Set([...visited, active]));
  }

  const previous = useRef(active);
  useEffect(() => {
    if (previous.current === active) {
      return;
    }
    previous.current = active;
    const panel = document.getElementById(`section-panel-${active}`);
    const target = panel?.querySelector<HTMLElement>("h1") ?? panel;
    target?.focus({ preventScroll: true });
  }, [active]);

  return (
    <OpenedReportProvider>
      {SECTIONS.filter((section) => visited.has(section.id)).map((section) => (
        <SectionPanel key={section.id} section={section} active={section.id === active} />
      ))}
    </OpenedReportProvider>
  );
}
