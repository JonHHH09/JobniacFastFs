import { copy } from "../../i18n/copy";
import { formatMessage, isMessageId } from "../../i18n/messages";
import type { FeatureDto, FeatureIdDto } from "../../ipc/bindings";
import { Button } from "../../ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../../ui/card";
import { Label } from "../../ui/label";
import { Spinner } from "../../ui/spinner";
import { StatusMessage } from "../../ui/status-message";
import { Switch } from "../../ui/switch";
import { useFeatures } from "./useFeatures";

/**
 * The features that expose a tool that reaches the network or deletes data to MCP clients. They are
 * off by default and shown apart.
 */
const MCP_CAPABILITIES: readonly FeatureIdDto[] = ["mcpArbeitnow", "mcpLinkFetch", "mcpDeletion"];

/**
 * The Settings card of the optional features: one switch each. The in-app features are on by
 * default; the three MCP capabilities (Arbeitnow, adding a job from a link, deleting) are off until
 * the user turns them on. A switch applies after a restart; a feature whose environment variable is set
 * cannot be changed here. It renders what Rust reports and sends the switch back.
 */
export function FeaturesCard() {
  const { query, setEnabled } = useFeatures();
  const data = query.data;

  if (!data) {
    return (
      <section aria-labelledby="features-heading" data-testid="featuresCard" className="grid">
        <Card>
          <CardHeader>
            <CardTitle>
              <h2 id="features-heading">{copy.features.title}</h2>
            </CardTitle>
          </CardHeader>
          <CardContent>
            {query.isError ? (
              <div className="grid justify-items-start gap-3">
                <StatusMessage tone="error">{copy.features.loadFailed}</StatusMessage>
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
      </section>
    );
  }

  const failure =
    setEnabled.error && !setEnabled.isPending
      ? formatMessage(
          isMessageId(setEnabled.error.messageId)
            ? setEnabled.error.messageId
            : "settings.saveFailed",
        )
      : null;

  const rows = (features: FeatureDto[]) =>
    features.map((feature) => (
      <FeatureRow
        key={feature.id}
        feature={feature}
        // A capability that is off may still be served by a server that could not be confirmed
        // stopped, so the row must not read as safely off.
        mayStillBeServed={
          data.stopIncomplete && MCP_CAPABILITIES.includes(feature.id) && !feature.enabled
        }
        pending={setEnabled.isPending}
        checked={
          feature.forcedByEnvironment
            ? feature.effective
            : setEnabled.isPending && setEnabled.variables?.id === feature.id
              ? setEnabled.variables.enabled
              : feature.enabled
        }
        onChange={(enabled) => {
          setEnabled.reset();
          setEnabled.mutate({ id: feature.id, enabled });
        }}
      />
    ));
  const inApp = data.features.filter((feature) => !MCP_CAPABILITIES.includes(feature.id));
  const mcp = data.features.filter((feature) => MCP_CAPABILITIES.includes(feature.id));

  return (
    <section aria-labelledby="features-heading" data-testid="featuresCard" className="grid">
      <Card>
        <CardHeader>
          <CardTitle>
            <h2 id="features-heading">{copy.features.title}</h2>
          </CardTitle>
        </CardHeader>
        <CardContent className="grid gap-5">
          <p className="text-muted-foreground">{copy.features.intro}</p>
          <ul className="grid gap-4">{rows(inApp)}</ul>
          {mcp.length > 0 ? (
            <section
              aria-labelledby="features-mcp-heading"
              data-testid="featuresMcpGroup"
              className="grid gap-3"
            >
              <h3 id="features-mcp-heading" className="text-sm font-medium">
                {copy.features.mcpTitle}
              </h3>
              <p className="text-[13px] text-muted-foreground">{copy.features.mcpIntro}</p>
              <p className="text-[13px] text-muted-foreground">{copy.features.mcpRevokeNote}</p>
              {data.stopIncomplete ? (
                <StatusMessage tone="error" data-testid="featuresStopIncomplete">
                  {copy.settings.stopIncompleteHint}
                </StatusMessage>
              ) : null}
              <ul className="grid gap-4">{rows(mcp)}</ul>
            </section>
          ) : null}
          <p className="text-[13px] text-muted-foreground">{copy.features.restartNote}</p>
          {failure ? <StatusMessage tone="error">{failure}</StatusMessage> : null}
        </CardContent>
      </Card>
    </section>
  );
}

function FeatureRow({
  feature,
  checked,
  pending,
  mayStillBeServed,
  onChange,
}: {
  feature: FeatureDto;
  checked: boolean;
  pending: boolean;
  mayStillBeServed: boolean;
  onChange: (enabled: boolean) => void;
}) {
  const text = copy.features.items[feature.id];
  const hintId = `feature-${feature.id}-hint`;
  // Rust tracks the switch as it was at startup: the change waits for the next start.
  const restartPending = !feature.forcedByEnvironment && feature.pendingRestart;
  // On, nothing waits for a restart, and still not running: it failed to start.
  const startFailed =
    feature.enabled &&
    !feature.effective &&
    !feature.pendingRestart &&
    !feature.forcedByEnvironment;
  return (
    <li className="grid gap-1" data-testid={`feature-${feature.id}`}>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <Switch
          id={`feature-switch-${feature.id}`}
          checked={checked}
          disabled={feature.forcedByEnvironment || pending}
          onCheckedChange={onChange}
          aria-describedby={hintId}
        />
        <Label htmlFor={`feature-switch-${feature.id}`} className="text-sm">
          {text.label}
        </Label>
      </div>
      <p id={hintId} className="text-[13px] text-muted-foreground">
        {text.hint}
        {feature.forcedByEnvironment ? ` ${copy.features.forced}` : null}
        {restartPending ? ` ${copy.features.restartPending}` : null}
        {startFailed ? ` ${copy.features.startFailed}` : null}
        {mayStillBeServed ? ` ${copy.features.stopIncompleteRow}` : null}
      </p>
    </li>
  );
}
