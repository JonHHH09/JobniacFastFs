import { useRef } from "react";
import { formatMessage, type MessageId } from "../../i18n/messages";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "../../ui/alert-dialog";
import { Button } from "../../ui/button";
import type { DeleteKind } from "./confirmation";

const TITLE_IDS: Record<DeleteKind, MessageId> = {
  profile: "delete.dialog.profileTitle",
  job: "delete.dialog.jobTitle",
};

/**
 * The reusable destructive confirmation of the profile and job delete.
 * Cancel is focused on open and is the default action: Escape cancels,
 * and an outside click does not close the dialog. The parent owns the state; `onCancel` runs for
 * Cancel and for Escape, so the caller clears its pending delete in both cases.
 */
export function ConfirmDeleteDialog({
  kind,
  confirming,
  message,
  onConfirm,
  onCancel,
}: {
  kind: DeleteKind;
  confirming: boolean;
  /** Plain text: rendered as a text node, never as markup. */
  message: string;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const cancelRef = useRef<HTMLButtonElement>(null);
  const prefix = kind === "profile" ? "deleteProfileDialog" : "deleteJobDialog";

  return (
    <AlertDialog
      open={confirming}
      onOpenChange={(next) => {
        // Only Escape reaches here as a close: the buttons below do not use the dialog's own actions.
        if (!next) {
          onCancel();
        }
      }}
    >
      <AlertDialogContent
        data-testid={prefix}
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          cancelRef.current?.focus();
        }}
      >
        <AlertDialogHeader>
          <AlertDialogTitle>{formatMessage(TITLE_IDS[kind])}</AlertDialogTitle>
          <AlertDialogDescription data-testid={`${prefix}Message`}>
            {message}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <Button
            ref={cancelRef}
            type="button"
            variant="outline"
            data-testid={`${prefix}CancelButton`}
            aria-label={formatMessage("delete.dialog.cancelAria")}
            onClick={onCancel}
          >
            {formatMessage("delete.dialog.cancel")}
          </Button>
          <Button
            type="button"
            variant="destructive"
            data-testid={`${prefix}ConfirmButton`}
            aria-label={formatMessage("delete.dialog.confirm")}
            onClick={onConfirm}
          >
            {formatMessage("delete.dialog.confirm")}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
