import { X } from "lucide-react";
import { useState } from "react";
import { copy } from "../../i18n/copy";
import { Button } from "../../ui/button";
import { UpdateInstall } from "./UpdateInstall";
import { useUpdateAvailableEvent, useUpdates } from "./useUpdates";

/**
 * The "update available" notice above every section. It shows what Rust found (the startup check
 * or the Settings button); nothing is requested here. Dismissing hides that version until the
 * next start.
 */
export function UpdateBanner() {
  useUpdateAvailableEvent();
  const { found } = useUpdates();
  const [dismissed, setDismissed] = useState<string | null>(null);

  if (!found?.available || !found.version || dismissed === found.version) {
    return null;
  }
  return (
    <section
      aria-label={copy.updates.bannerTitle(found.version)}
      data-testid="updateBanner"
      className="grid w-full max-w-[1200px] gap-3 rounded-panel border border-border bg-card p-4"
    >
      <div className="flex items-start justify-between gap-3">
        <p className="font-medium">{copy.updates.bannerTitle(found.version)}</p>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label={copy.updates.dismiss}
          onClick={() => setDismissed(found.version)}
        >
          <X strokeWidth={1.75} aria-hidden />
        </Button>
      </div>
      <UpdateInstall update={found} />
    </section>
  );
}
