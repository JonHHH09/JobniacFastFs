import type { FeatureIdDto, FeaturesDto } from "../../ipc/bindings";
import type { Args, HandlerMap } from "./util";

// Synthetic, stateful stand-in for the feature commands. The in-app features start on and the three
// MCP capabilities start off, like the app; Arbeitnow stands in for one forced by an environment
// variable (its switch is disabled). A change is saved but leaves the effective state alone, like
// the app, which applies it after a restart; turning off an MCP capability applies at once, and a
// capability that was revoked does not come back before the next start.
const IDS: FeatureIdDto[] = [
  "arbeitnow",
  "linkFetch",
  "deletion",
  "documentArtifacts",
  "generation",
  "mcpArbeitnow",
  "mcpLinkFetch",
  "mcpDeletion",
];
const OFF_BY_DEFAULT: FeatureIdDto[] = ["mcpArbeitnow", "mcpLinkFetch", "mcpDeletion"];
const FORCED: FeatureIdDto = "arbeitnow";

const enabled = new Map<FeatureIdDto, boolean>(IDS.map((id) => [id, !OFF_BY_DEFAULT.includes(id)]));
const startedOn = new Map<FeatureIdDto, boolean>(enabled);
// The MCP capabilities revoked in this run: usable no more, and they stay off until the next start.
const revoked = new Set<FeatureIdDto>();

function snapshot(): FeaturesDto {
  return {
    stopIncomplete: false,
    features: IDS.map((id) => {
      const on = enabled.get(id) ?? false;
      const started = startedOn.get(id) ?? false;
      const mcp = OFF_BY_DEFAULT.includes(id);
      return {
        id,
        enabled: on,
        effective: started && !revoked.has(id),
        forcedByEnvironment: id === FORCED,
        pendingRestart: mcp ? on && (!started || revoked.has(id)) : on !== started,
      };
    }),
  };
}

export const featureHandlers: HandlerMap = {
  features_get: (): FeaturesDto => snapshot(),
  feature_set_enabled: (args: Args): FeaturesDto => {
    const id = IDS.find((candidate) => candidate === args.id);
    if (id) {
      const on = args.enabled === true;
      enabled.set(id, on);
      if (!on && OFF_BY_DEFAULT.includes(id) && startedOn.get(id)) {
        revoked.add(id);
      }
    }
    return snapshot();
  },
};
