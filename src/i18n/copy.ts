/**
 * UI copy catalogue. Strings render as text nodes only; the Rust side sends message ids, never
 * copy, so every user-facing sentence lives here.
 */
export const copy = {
  appName: "JobniacFast",
  nav: {
    skipToContent: "Skip to content",
    main: "Main navigation",
    health: "Health",
    settings: "Settings",
    openMenu: "Open navigation menu",
    closeMenu: "Close navigation menu",
    menuTitle: "Navigation",
    unsavedChanges: "unsaved changes",
    groupCandidates: "Candidates",
    groupJobs: "Jobs",
    groupMatching: "Matching",
    groupDocuments: "Documents",
    groupSystem: "System",
  },
  // Section pages of the shell: the nav label doubles as the page title.
  sections: {
    profiles: {
      label: "Profiles",
      description: "Create, edit and search the candidate profiles you match against.",
    },
    profilePdf: {
      label: "Profile from PDF",
      description: "Fill a profile with the details found in a stored PDF.",
    },
    jobs: {
      label: "Saved jobs",
      description: "Save job postings, edit them and find them again.",
    },
    arbeitnow: {
      label: "Arbeitnow",
      description: "Scan the Arbeitnow job board for postings to save.",
    },
    matching: {
      label: "Matches",
      description: "Compare a profile with a job and open the saved match reports.",
    },
    reviews: {
      label: "Reviews",
      description: "Ask for an advisory review of the match report you opened.",
    },
    documents: {
      label: "PDF documents",
      description: "Import PDF documents and read their stored pages.",
    },
    artifacts: {
      label: "Stored PDFs",
      description: "Preview the PDFs the app has stored for you.",
    },
    generation: {
      label: "Generate",
      description: "Write the text of a document and generate a PDF from it.",
    },
    health: { label: "Health" },
    settings: {
      label: "Settings",
      description: "Choose how this app runs on your computer.",
    },
  },
  theme: {
    label: "Theme",
    system: "System",
    light: "Light",
    dark: "Dark",
  },
  status: {
    success: "Success: ",
    error: "Error: ",
    warning: "Warning: ",
  },
  shell: {
    loadingSection: "Loading section",
    dismissNotice: "Dismiss notice",
  },
  health: {
    toolbar: "Health status toolbar",
    title: "Health",
    description: "Check that the database and the shared services respond.",
    statusHeading: "Health status",
    refresh: "Refresh health",
    refreshing: "Checking…",
    statusNone: "Not checked",
    statusAria: (text: string) => `Health status: ${text}`,
    explainChecking: "The check runs outside the interface so this window stays responsive.",
    explainIdle: "Use Refresh health to request the current result.",
    explainFailed: (category: string) => `The health check failed: ${category}`,
    checkDetails: "Check details",
    checkedAt: "Checked at",
    successful: "Successful checks",
    failed: "Failed checks",
    notAvailable: "Not available",
    cleanupTitle: "Generated resume cleanup",
    cleanupUnknown: "Cleanup status is not available yet.",
    cleanupStatus: (status: string) => `Cleanup status: ${status}`,
    cleanupNotAvailable: "Cleanup status: Not available",
    footer: "This screen shares the health check with MCP.",
  },
  settings: {
    mcpTitle: "MCP server",
    mcpIntro:
      "Let an MCP client on this computer use Jobniac. The server listens on this computer only (loopback), never on the network.",
    switchLabel: "Run the MCP server",
    statusLabel: "Status",
    status: {
      stopped: "Stopped",
      starting: "Starting…",
      running: "Running",
      failed: "Failed",
      unavailable: "Unavailable",
    },
    startingHint: "Starting the server can take a few seconds.",
    failedHint: "The server did not start. Check the port, then turn it on again.",
    unavailableHint:
      "The server program is missing. Release builds include it; in a development build, run cargo build -p jobniac-cli first.",
    portLabel: "Port",
    portHint: "A port from 1024 to 65535. Change it while the server is stopped.",
    portSave: "Save port",
    copy: "Copy connection details",
    copied: "Copied",
    copyHint:
      "A new access token is created each time the server starts. Paste the copied details into your MCP client.",
    loading: "Loading settings…",
    loadFailed: "Unable to load settings. Please try again.",
    retry: "Try again",
  },
} as const;
