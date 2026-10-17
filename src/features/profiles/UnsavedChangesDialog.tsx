import type { MessageId } from "../../i18n/messages";
import { formatMessage } from "../../i18n/messages";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "../../ui/alert-dialog";

/**
 * The unsaved-changes confirmation. Escape and an outside click keep the editing
 * state (the Radix default for an alert dialog is to stay open until one of the two buttons runs).
 */
export function UnsavedChangesDialog({
  open,
  message,
  onKeep,
  onDiscard,
}: {
  open: boolean;
  message: MessageId;
  onKeep: () => void;
  onDiscard: () => void;
}) {
  return (
    <AlertDialog open={open} onOpenChange={(next) => (next ? undefined : onKeep())}>
      <AlertDialogContent data-testid="unsavedChangesDialog">
        <AlertDialogHeader>
          <AlertDialogTitle>{formatMessage("profile.confirm.discardTitle")}</AlertDialogTitle>
          <AlertDialogDescription data-testid="unsavedChangesMessage">
            {formatMessage(message)}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel
            data-testid="cancelUnsavedActionButton"
            aria-label={formatMessage("ui.dialog.keepAria")}
            onClick={onKeep}
          >
            {formatMessage("ui.dialog.keep")}
          </AlertDialogCancel>
          <AlertDialogAction
            data-testid="confirmUnsavedActionButton"
            aria-label={formatMessage("ui.dialog.discardAria")}
            onClick={onDiscard}
          >
            {formatMessage("ui.dialog.discard")}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
