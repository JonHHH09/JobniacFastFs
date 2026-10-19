import { formatMessage } from "../../i18n/messages";
import type { SourceViewDto } from "../../ipc/bindings";
import { formatInstant, formatRelative } from "../../lib/format";
import { useProfileSource } from "./queries";

/** The identifier lines of a profile's PDF source: raw values, shown only in a details block. */
export function sourceIdLines(source: SourceViewDto): string[] {
  return [
    formatMessage("profilePdf.source.sourceLinkId", { sourceLinkId: source.sourceLinkId }),
    formatMessage("profilePdf.source.profileId", { profileId: source.profileId }),
    formatMessage("profilePdf.source.extractionId", { extractionId: source.extractionId }),
  ];
}

/** The readable lines of a profile's PDF source: the source type and when it was created. */
export function sourceLines(source: SourceViewDto): string[] {
  const relative = formatRelative(source.createdAt);
  const created = formatInstant(source.createdAt);
  return [
    formatMessage("profilePdf.source.type"),
    formatMessage("profilePdf.source.createdAt", {
      createdAt: relative === "" ? created : `${created} (${relative})`,
    }),
  ];
}

/**
 * A read-only note under the profile fields saying the profile was built from a stored PDF. It
 * renders nothing for a profile without a source (or when the lookup is refused).
 */
export function ProfileSourceNote({ profileId }: { profileId: string | null }) {
  const source = useProfileSource(profileId);
  if (source === null) {
    return null;
  }
  return (
    <div data-testid="profileSourceNote" className="grid gap-1 p-4 text-[13px]">
      <h3 className="text-sm font-semibold">{formatMessage("profilePdf.source.heading")}</h3>
      <ul className="grid gap-1 text-muted-foreground [overflow-wrap:anywhere]">
        {sourceLines(source).map((line) => (
          <li key={line}>{line}</li>
        ))}
      </ul>
      <details>
        <summary className="cursor-pointer text-muted-foreground">
          {formatMessage("profilePdf.details")}
        </summary>
        <ul className="mt-1 grid gap-1 font-mono text-xs [overflow-wrap:anywhere]">
          {sourceIdLines(source).map((line) => (
            <li key={line}>{line}</li>
          ))}
        </ul>
      </details>
    </div>
  );
}
