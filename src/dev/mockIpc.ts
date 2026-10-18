import { mockIPC } from "@tauri-apps/api/mocks";
import { arbeitnowHandlers } from "./mocks/arbeitnow";
import { artifactHandlers } from "./mocks/artifacts";
import { deletionHandlers } from "./mocks/deletion";
import { documentHandlers } from "./mocks/documents";
import { generationHandlers } from "./mocks/generation";
import { healthHandlers } from "./mocks/health";
import { jobHandlers } from "./mocks/jobs";
import { matchingHandlers } from "./mocks/matching";
import { profilePdfHandlers } from "./mocks/profilePdf";
import { profileHandlers } from "./mocks/profiles";
import { reviewHandlers } from "./mocks/reviews";
import { settingsHandlers } from "./mocks/settings";
import { shellHandlers } from "./mocks/shell";
import { updateHandlers } from "./mocks/updates";
import { type Args, type HandlerMap, injectedFailure, mockParams } from "./mocks/util";

// Development-only. Imported from src/main.tsx behind `import.meta.env.DEV`, so a production build
// never contains it. See src/dev/README.md.

/** Every mocked command, assembled from the per-feature modules in `./mocks`. */
export const handlers: HandlerMap = {
  ...profileHandlers,
  ...jobHandlers,
  ...matchingHandlers,
  ...reviewHandlers,
  ...documentHandlers,
  ...artifactHandlers,
  ...generationHandlers,
  ...profilePdfHandlers,
  ...arbeitnowHandlers,
  ...healthHandlers,
  ...deletionHandlers,
  ...shellHandlers,
  ...settingsHandlers,
  ...updateHandlers,
};

const DEFAULT_LATENCY_MS = 150;

function latencyMs(): number {
  const raw = mockParams.get("mockLatency");
  const value = raw === null ? Number.NaN : Number(raw);
  return Number.isFinite(value) && value >= 0 ? value : DEFAULT_LATENCY_MS;
}

function failingCommands(): Set<string> {
  return new Set(
    mockParams
      .getAll("mockFail")
      .flatMap((value) => value.split(","))
      .map((name) => name.trim())
      .filter((name) => name !== ""),
  );
}

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** Installs the mock IPC on `window`. Commands, events and URL switches are described in the README. */
export function installMockIpc(): void {
  const delay = latencyMs();
  const failing = failingCommands();
  const warned = new Set<string>();
  mockIPC(
    async (cmd, payload) => {
      if (delay > 0) {
        await sleep(delay);
      }
      if (failing.has(cmd)) {
        return injectedFailure();
      }
      const handler = handlers[cmd];
      if (!handler) {
        if (!warned.has(cmd)) {
          warned.add(cmd);
          console.warn(`[mock-ipc] no mock for command "${cmd}"`);
        }
        throw { code: "internal_error", messageId: "" };
      }
      return handler((payload ?? {}) as Args);
    },
    { shouldMockEvents: true },
  );
  console.info(
    `[mock-ipc] installed: ${Object.keys(handlers).length} commands, ${delay} ms latency` +
      (failing.size > 0 ? `, failing: ${[...failing].join(", ")}` : ""),
  );
}
