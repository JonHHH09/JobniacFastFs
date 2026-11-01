import { Check, Copy } from "lucide-react";
import { type FormEvent, useEffect, useRef, useState } from "react";
import { copy } from "../../i18n/copy";
import { formatMessage, isMessageId, type MessageId } from "../../i18n/messages";
import type { CommandError, McpServerStatusDto } from "../../ipc/bindings";
import { Badge } from "../../ui/badge";
import { Button } from "../../ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../../ui/card";
import { Field } from "../../ui/field";
import { Input } from "../../ui/input";
import { Label } from "../../ui/label";
import { Spinner } from "../../ui/spinner";
import { StatusMessage } from "../../ui/status-message";
import { Switch } from "../../ui/switch";
import { UpdatesCard } from "../updates/UpdatesCard";
import { FeaturesCard } from "./FeaturesCard";
import { useMcpServer } from "./useMcpServer";

// The ranges are a convenience for the form only: Rust validates the port and its answer is final.
const MIN_PORT = 1024;
const MAX_PORT = 65535;
const COPIED_MS = 2000;

const badgeVariant = {
  stopped: "neutral",
  starting: "outline",
  running: "success",
  failed: "destructive",
  unavailable: "warning",
} as const satisfies Record<McpServerStatusDto, string>;

/** The port as a number, or null when the text is not a port. */
function parsePort(text: string): number | null {
  if (!/^\d{1,5}$/.test(text)) {
    return null;
  }
  const port = Number(text);
  return port >= MIN_PORT && port <= MAX_PORT ? port : null;
}

/**
 * The catalogue copy for a failed command, never the raw error. A busy refusal carries no copy (the
 * controls are locked meanwhile); an id without copy falls back to the operation's own text.
 */
function errorText(error: CommandError, fallback: MessageId): string | null {
  if (error.code === "busy") {
    return null;
  }
  return formatMessage(isMessageId(error.messageId) ? error.messageId : fallback);
}

function statusHint(status: McpServerStatusDto): string | null {
  switch (status) {
    case "starting":
      return copy.settings.startingHint;
    case "failed":
      return copy.settings.failedHint;
    case "unavailable":
      return copy.settings.unavailableHint;
    default:
      return null;
  }
}

/** The settings screen: a card for the MCP server, the optional features and updates. */
export function SettingsPage() {
  return (
    <div className="grid gap-[18px]">
      <McpServerCard />
      <FeaturesCard />
      <UpdatesCard />
    </div>
  );
}

/** The MCP server card. It renders what Rust reports and sends input back. */
function McpServerCard() {
  const { query, setEnabled, setPort, copyConnection } = useMcpServer();
  const [portDraft, setPortDraft] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const copiedTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  useEffect(() => () => clearTimeout(copiedTimer.current), []);

  const settings = query.data;
  const currentStatus = settings?.mcp.status;
  const resetCopy = copyConnection.reset;
  // A copy error belongs to a running server: drop it once the status moves away.
  useEffect(() => {
    if (currentStatus !== "running") {
      resetCopy();
    }
  }, [currentStatus, resetCopy]);

  if (!settings) {
    return (
      <Card>
        <CardContent>
          {query.isError ? (
            <div className="grid justify-items-start gap-3">
              <StatusMessage tone="error">{copy.settings.loadFailed}</StatusMessage>
              <Button type="button" variant="outline" onClick={() => void query.refetch()}>
                {copy.settings.retry}
              </Button>
            </div>
          ) : (
            <p role="status" className="flex items-center gap-2 text-muted-foreground">
              <Spinner />
              {copy.settings.loading}
            </p>
          )}
        </CardContent>
      </Card>
    );
  }

  const { enabled, port, status } = settings.mcp;
  const changing = setEnabled.isPending || setPort.isPending;
  const busy = changing || status === "starting";
  const checked = setEnabled.isPending ? setEnabled.variables === true : enabled;
  const portLocked = busy || status === "running";
  const draft = portDraft ?? String(port);
  const parsed = parsePort(draft);
  const portInvalid = portDraft !== null && parsed === null;
  const portChanged = portDraft !== null && portDraft !== String(port);
  const hint = statusHint(status);

  const failure =
    setEnabled.error && !setEnabled.isPending
      ? errorText(setEnabled.error, "settings.mcp.failed")
      : setPort.error && !setPort.isPending
        ? errorText(setPort.error, "settings.saveFailed")
        : copyConnection.error
          ? errorText(copyConnection.error, "settings.mcp.clipboardFailed")
          : null;

  function toggle(next: boolean) {
    setPort.reset();
    copyConnection.reset();
    // The typed draft stays unless Rust answers with a different port.
    setEnabled.mutate(next, {
      onSuccess: (answer) => {
        if (answer.mcp.port !== port) {
          setPortDraft(null);
        }
      },
    });
  }

  function savePort(event: FormEvent) {
    event.preventDefault();
    if (parsed === null || !portChanged || portLocked) {
      return;
    }
    setEnabled.reset();
    setPort.mutate(parsed, { onSuccess: () => setPortDraft(null) });
  }

  function copyDetails() {
    setCopied(false);
    clearTimeout(copiedTimer.current);
    copyConnection.mutate(undefined, {
      onSuccess: () => {
        setCopied(true);
        copiedTimer.current = setTimeout(() => setCopied(false), COPIED_MS);
      },
    });
  }

  return (
    <section aria-labelledby="mcp-server-heading" data-testid="mcpServerCard" className="grid">
      <Card>
        <CardHeader>
          <CardTitle>
            <h2 id="mcp-server-heading">{copy.settings.mcpTitle}</h2>
          </CardTitle>
        </CardHeader>
        <CardContent className="grid gap-5">
          <p className="text-muted-foreground">{copy.settings.mcpIntro}</p>

          <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
            <Switch
              id="mcp-enabled"
              checked={checked}
              disabled={busy}
              onCheckedChange={toggle}
              aria-describedby={hint ? "mcp-status-hint" : undefined}
            />
            <Label htmlFor="mcp-enabled" className="text-sm">
              {copy.settings.switchLabel}
            </Label>
            <Badge
              variant={badgeVariant[status]}
              role="status"
              aria-live="polite"
              data-testid="mcpStatus"
              data-status={status}
            >
              {busy ? <Spinner className="size-3 border-[1.5px]" /> : null}
              <span className="sr-only">{copy.settings.statusLabel}: </span>
              {copy.settings.status[status]}
            </Badge>
          </div>
          {hint ? (
            <p id="mcp-status-hint" className="text-[13px] text-muted-foreground">
              {hint}
            </p>
          ) : null}

          <form onSubmit={savePort} className="flex flex-wrap items-start gap-3" noValidate>
            <Field
              label={copy.settings.portLabel}
              hint={copy.settings.portHint}
              error={portInvalid ? formatMessage("settings.mcp.invalidPort") : undefined}
              className="w-48"
            >
              {(control) => (
                <Input
                  {...control}
                  inputMode="numeric"
                  autoComplete="off"
                  maxLength={5}
                  value={draft}
                  disabled={portLocked}
                  onChange={(event) => setPortDraft(event.target.value.replace(/\D/g, ""))}
                />
              )}
            </Field>
            <Button
              type="submit"
              variant="outline"
              className="mt-5"
              disabled={portLocked || !portChanged || portInvalid}
            >
              {copy.settings.portSave}
            </Button>
          </form>

          {status === "running" ? (
            <div className="grid justify-items-start gap-2">
              <Button
                type="button"
                variant="secondary"
                onClick={copyDetails}
                disabled={copyConnection.isPending}
              >
                {copied ? (
                  <Check strokeWidth={1.75} aria-hidden />
                ) : (
                  <Copy strokeWidth={1.75} aria-hidden />
                )}
                {copied ? copy.settings.copied : copy.settings.copy}
              </Button>
              <p className="text-[13px] text-muted-foreground">{copy.settings.copyHint}</p>
            </div>
          ) : null}

          {failure ? <StatusMessage tone="error">{failure}</StatusMessage> : null}
        </CardContent>
      </Card>
    </section>
  );
}
