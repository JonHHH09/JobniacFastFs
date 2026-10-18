import { useRef, useState } from "react";
import { copy } from "../../i18n/copy";
import { formatMessage, isMessageId } from "../../i18n/messages";
import type { UpdateCheckDto } from "../../ipc/bindings";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "../../ui/alert-dialog";
import { Button } from "../../ui/button";
import { Spinner } from "../../ui/spinner";
import { StatusMessage } from "../../ui/status-message";
import { RELEASES_URL, useUpdates } from "./useUpdates";

/**
 * What to do about an available update: the install button behind a confirmation, or, for a copy
 * installed from a `.deb`, the releases page (the package manager owns those files). Rust refuses
 * the install for a package copy as well; the UI only hides the button.
 */
export function UpdateInstall({ update }: { update: UpdateCheckDto }) {
  const { install, installing, installError } = useUpdates();
  const [confirming, setConfirming] = useState(false);
  const cancelRef = useRef<HTMLButtonElement>(null);

  if (update.installKind === "deb") {
    return (
      <div className="grid gap-1 text-[13px]" data-testid="updateDebHint">
        <p className="text-muted-foreground">{copy.updates.debHint}</p>
        <p>
          <a
            href={RELEASES_URL}
            target="_blank"
            rel="noreferrer noopener"
            className="underline underline-offset-2"
          >
            {copy.updates.releasesLink}
          </a>{" "}
          <span className="text-muted-foreground [overflow-wrap:anywhere]">{RELEASES_URL}</span>
        </p>
      </div>
    );
  }

  // A busy refusal carries no copy (the buttons are locked meanwhile), like the Settings card.
  const failure =
    installError && installError.code !== "busy"
      ? formatMessage(
          isMessageId(installError.messageId) ? installError.messageId : "update.installFailed",
        )
      : null;

  return (
    <div className="grid justify-items-start gap-2">
      <Button
        type="button"
        data-testid="updateInstallButton"
        disabled={installing}
        onClick={() => setConfirming(true)}
      >
        {installing ? <Spinner className="size-3 border-[1.5px]" /> : null}
        {installing ? copy.updates.installing : copy.updates.installButton}
      </Button>
      {failure ? <StatusMessage tone="error">{failure}</StatusMessage> : null}
      <AlertDialog open={confirming} onOpenChange={setConfirming}>
        <AlertDialogContent
          data-testid="updateInstallDialog"
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            cancelRef.current?.focus();
          }}
        >
          <AlertDialogHeader>
            <AlertDialogTitle>{copy.updates.dialogTitle(update.version ?? "")}</AlertDialogTitle>
            <AlertDialogDescription>{copy.updates.dialogBody}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <Button
              ref={cancelRef}
              type="button"
              variant="outline"
              onClick={() => setConfirming(false)}
            >
              {copy.updates.dialogCancel}
            </Button>
            <Button
              type="button"
              data-testid="updateInstallConfirm"
              onClick={() => {
                setConfirming(false);
                if (update.version) {
                  install.mutate(update.version);
                }
              }}
            >
              {copy.updates.dialogConfirm}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
