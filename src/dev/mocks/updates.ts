import type { UpdateCheckDto, UpdateSettingsDto } from "../../ipc/bindings";
import { type Args, fail, type HandlerMap, mockParams } from "./util";

// Synthetic, stateful stand-in for the updater commands. `?mockUpdate=1` makes a check find
// version 9.9.9, `?mockUpdate=deb` does so as a package install, and `?mockUpdate=off` reports a
// build without updater configuration.
const mode = mockParams.get("mockUpdate");
const installKind = mode === "deb" ? "deb" : "installer";
const state = { autoCheck: false };

function announced(): UpdateCheckDto {
  const available = mode === "1" || mode === "deb";
  return {
    available,
    version: available ? "9.9.9" : null,
    notes: available ? "### Added\n- Synthetic release notes." : null,
    installKind,
  };
}

function snapshot(): UpdateSettingsDto {
  return {
    autoCheck: state.autoCheck,
    supported: mode !== "off",
    installKind,
    pending: null,
  };
}

export const updateHandlers: HandlerMap = {
  update_settings_get: (): UpdateSettingsDto => snapshot(),
  update_set_auto_check: (args: Args): UpdateSettingsDto => {
    state.autoCheck = args.enabled === true;
    return snapshot();
  },
  check_for_update: (): UpdateCheckDto => {
    if (mode === "off") {
      return fail("not_configured", "update.unavailable");
    }
    return announced();
  },
  install_update: (): null => fail("internal_error", "update.installFailed"),
};
