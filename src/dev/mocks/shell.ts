import type { HandlerMap } from "./util";

// The close gate lives in Rust; a browser has no window to close, so these only acknowledge.
export const shellHandlers: HandlerMap = {
  shell_set_dirty: () => null,
  shell_request_close: () => null,
};
