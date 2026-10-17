import { QueryClient } from "@tanstack/react-query";

/**
 * The shared query client. Health checks are not retried: a failure is shown as the failed state,
 * and nothing is re-run in the background.
 */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: false,
      refetchOnWindowFocus: false,
    },
  },
});
