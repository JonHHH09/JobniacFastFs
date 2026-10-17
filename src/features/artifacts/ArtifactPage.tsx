import { useEffect, useState } from "react";
import { formatMessage } from "../../i18n/messages";
import type { ArtifactCapabilitiesDto } from "../../ipc/bindings";
import { ArtifactPreview } from "./ArtifactPreview";
import { fetchArtifactCapabilities } from "./api";
import { NOT_ADMITTED } from "./artifact";

/**
 * The "View or save a stored PDF" screen. It is always shown; its controls are
 * disabled until Rust admits the artifact services, and the Save control appears only when the export
 * is admitted too.
 */
export function ArtifactPage() {
  const [capabilities, setCapabilities] = useState<ArtifactCapabilitiesDto | null>(null);

  useEffect(() => {
    let active = true;
    fetchArtifactCapabilities().then(
      (answer) => {
        if (active) {
          setCapabilities(answer);
        }
      },
      () => {
        if (active) {
          setCapabilities(NOT_ADMITTED);
        }
      },
    );
    return () => {
      active = false;
    };
  }, []);

  return (
    <section
      aria-labelledby="artifacts-heading"
      data-testid="artifacts-section"
      className="@container min-w-0"
    >
      <h2 id="artifacts-heading" className="sr-only">
        {formatMessage("artifacts.title")}
      </h2>
      <p className="sr-only">{formatMessage("artifacts.a11y.title")}</p>
      <ArtifactPreview
        available={capabilities?.artifacts === true}
        save={capabilities?.export === true}
      />
    </section>
  );
}
