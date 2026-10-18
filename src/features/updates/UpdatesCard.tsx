import { copy } from "../../i18n/copy";
import { formatMessage, isMessageId, type MessageId } from "../../i18n/messages";
import type { CommandError } from "../../ipc/bindings";
import { Button } from "../../ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../../ui/card";
import { Label } from "../../ui/label";
import { Spinner } from "../../ui/spinner";
import { StatusMessage } from "../../ui/status-message";
import { Switch } from "../../ui/switch";
import { UpdateInstall } from "./UpdateInstall";
import { useUpdates } from "./useUpdates";

/** The catalogue copy for a failed command, never the raw error. A busy refusal carries none. */
function errorText(error: CommandError, fallback: MessageId): string | null {
  if (error.code === "busy") {
    return null;
  }
  return formatMessage(isMessageId(error.messageId) ? error.messageId : fallback);
}

/**
 * The Settings card of the updater: the check button, the auto-check switch (off by default) and
 * the status text. It renders what Rust reports; a build without updater configuration says so and
 * offers no check.
 */
export function UpdatesCard() {
  const { settings, setAutoCheck, check, found, checked } = useUpdates();
  const data = settings.data;

  // A settings read that failed hides the card: the MCP card above already reports the load error.
  if (!data) {
    return null;
  }

  const autoChecked = setAutoCheck.isPending ? setAutoCheck.variables === true : data.autoCheck;
  const failure = check.error
    ? errorText(check.error, "update.checkFailed")
    : setAutoCheck.error && !setAutoCheck.isPending
      ? errorText(setAutoCheck.error, "settings.saveFailed")
      : null;

  let status: string | null = null;
  if (!data.supported) {
    status = copy.updates.unsupported;
  } else if (found?.available && found.version) {
    status = copy.updates.available(found.version);
  } else if (checked && !checked.available) {
    status = copy.updates.upToDate;
  }

  return (
    <section aria-labelledby="updates-heading" data-testid="updatesCard" className="grid">
      <Card>
        <CardHeader>
          <CardTitle>
            <h2 id="updates-heading">{copy.updates.title}</h2>
          </CardTitle>
        </CardHeader>
        <CardContent className="grid gap-5">
          <p className="text-muted-foreground">{copy.updates.intro}</p>

          <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
            <Switch
              id="update-auto-check"
              checked={autoChecked}
              disabled={!data.supported || setAutoCheck.isPending}
              onCheckedChange={(next) => {
                check.reset();
                setAutoCheck.mutate(next);
              }}
              aria-describedby="update-auto-check-hint"
            />
            <Label htmlFor="update-auto-check" className="text-sm">
              {copy.updates.autoCheckLabel}
            </Label>
          </div>
          <p id="update-auto-check-hint" className="text-[13px] text-muted-foreground">
            {copy.updates.autoCheckHint}
          </p>

          <div className="grid justify-items-start gap-2">
            <Button
              type="button"
              variant="secondary"
              data-testid="updateCheckButton"
              disabled={!data.supported || check.isPending}
              onClick={() => {
                setAutoCheck.reset();
                check.mutate();
              }}
            >
              {check.isPending ? <Spinner className="size-3 border-[1.5px]" /> : null}
              {check.isPending ? copy.updates.checking : copy.updates.checkButton}
            </Button>
            {status ? (
              <StatusMessage tone="neutral" data-testid="updateStatus">
                {status}
              </StatusMessage>
            ) : null}
          </div>

          {found?.available && found.notes ? (
            <div className="grid gap-1">
              <p className="text-sm font-medium">{copy.updates.notesLabel}</p>
              <pre
                data-testid="updateNotes"
                className="max-h-48 overflow-auto whitespace-pre-wrap text-[13px] text-muted-foreground [overflow-wrap:anywhere]"
              >
                {found.notes}
              </pre>
            </div>
          ) : null}
          {found?.available ? <UpdateInstall update={found} /> : null}

          {failure ? <StatusMessage tone="error">{failure}</StatusMessage> : null}
        </CardContent>
      </Card>
    </section>
  );
}
