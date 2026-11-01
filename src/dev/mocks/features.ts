import type { FeatureIdDto, FeaturesDto } from "../../ipc/bindings";
import type { Args, HandlerMap } from "./util";

// Synthetic, stateful stand-in for the feature commands. Every feature starts on; Arbeitnow stands
// in for one forced by an environment variable (its switch is disabled). A change is saved but
// leaves the effective state alone, like the app, which applies it after a restart.
const IDS: FeatureIdDto[] = [
  "arbeitnow",
  "linkFetch",
  "deletion",
  "documentArtifacts",
  "generation",
];
const FORCED: FeatureIdDto = "arbeitnow";

const enabled = new Map<FeatureIdDto, boolean>(IDS.map((id) => [id, true]));

function snapshot(): FeaturesDto {
  return {
    features: IDS.map((id) => ({
      id,
      enabled: enabled.get(id) ?? true,
      effective: true,
      forcedByEnvironment: id === FORCED,
      pendingRestart: false,
    })),
  };
}

export const featureHandlers: HandlerMap = {
  features_get: (): FeaturesDto => snapshot(),
  feature_set_enabled: (args: Args): FeaturesDto => {
    const id = IDS.find((candidate) => candidate === args.id);
    if (id) {
      enabled.set(id, args.enabled === true);
    }
    return snapshot();
  },
};
