import "@fontsource-variable/schibsted-grotesk";
import { QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import React from "react";
import ReactDOM from "react-dom/client";
import { queryClient } from "./app/query-client";
import { router } from "./app/router";
import { initTheme } from "./app/theme";
import "./styles.css";

// The Content Security Policy forbids inline scripts, so the stored theme is applied here, before
// the first render, instead of in index.html.
initTheme();

// Development only: `pnpm dev:mock` serves the UI in a browser against synthetic data (src/dev).
// The condition is a build-time constant, so a production build drops the import entirely.
if (import.meta.env.DEV && import.meta.env.VITE_MOCK_IPC === "1") {
  const { installMockIpc } = await import("./dev/mockIpc");
  installMockIpc();
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </React.StrictMode>,
);
