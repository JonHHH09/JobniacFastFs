import {
  Activity,
  Briefcase,
  Files,
  FileText,
  FileUser,
  Globe,
  type LucideIcon,
  MessageSquareText,
  Scale,
  Settings,
  Sparkles,
  UserRound,
} from "lucide-react";
import { type ComponentType, lazy } from "react";
import { copy } from "../i18n/copy";
import type { ShellDirtyWorkspace } from "../ipc/bindings";

/** Every section of the shell, in navigation order. The id is the `?section=` value. */
export const SECTION_IDS = [
  "profiles",
  "profilePdf",
  "jobs",
  "arbeitnow",
  "matching",
  "reviews",
  "documents",
  "artifacts",
  "generation",
  "health",
  "settings",
] as const;

export type SectionId = (typeof SECTION_IDS)[number];

export const DEFAULT_SECTION: SectionId = "profiles";

export type SectionGroup = {
  id: string;
  label: string;
  sections: readonly SectionDefinition[];
};

export type SectionDefinition = {
  id: SectionId;
  label: string;
  /** One plain sentence under the page title. The health section has none: its toolbar is the header. */
  description?: string;
  icon: LucideIcon;
  /** Lists get the wide content column, forms the readable one. */
  width: "form" | "wide";
  /** The close-gate workspace whose unsaved edits mark this item in the navigation. */
  dirtyWorkspace?: ShellDirtyWorkspace;
  component: ComponentType;
};

// One lazy chunk per section: a section is fetched the first time it is visited.
const ProfilesPage = lazy(() =>
  import("../features/profiles/ProfilesPage").then((m) => ({ default: m.ProfilesPage })),
);
const ProfilePdfPage = lazy(() =>
  import("../features/profilePdf/ProfilePdfPage").then((m) => ({ default: m.ProfilePdfPage })),
);
const JobsPage = lazy(() =>
  import("../features/jobs/JobsPage").then((m) => ({ default: m.JobsPage })),
);
const ArbeitnowPage = lazy(() =>
  import("../features/arbeitnow/ArbeitnowPage").then((m) => ({ default: m.ArbeitnowPage })),
);
const MatchingPage = lazy(() =>
  import("../features/matching/MatchingPage").then((m) => ({ default: m.MatchingPage })),
);
const ReviewsPage = lazy(() =>
  import("../features/reviews/ReviewsPage").then((m) => ({ default: m.ReviewsPage })),
);
const DocumentsPage = lazy(() =>
  import("../features/documents/DocumentsPage").then((m) => ({ default: m.DocumentsPage })),
);
const ArtifactPage = lazy(() =>
  import("../features/artifacts/ArtifactPage").then((m) => ({ default: m.ArtifactPage })),
);
const GenerationPage = lazy(() =>
  import("../features/generation/GenerationPage").then((m) => ({ default: m.GenerationPage })),
);
const HealthPage = lazy(() =>
  import("../features/health/HealthPage").then((m) => ({ default: m.HealthPage })),
);
const SettingsPage = lazy(() =>
  import("../features/settings/SettingsPage").then((m) => ({ default: m.SettingsPage })),
);

const s = copy.sections;

export const SECTION_GROUPS: readonly SectionGroup[] = [
  {
    id: "candidates",
    label: copy.nav.groupCandidates,
    sections: [
      {
        id: "profiles",
        ...s.profiles,
        icon: UserRound,
        width: "wide",
        dirtyWorkspace: "profile",
        component: ProfilesPage,
      },
      {
        id: "profilePdf",
        ...s.profilePdf,
        icon: FileUser,
        width: "form",
        component: ProfilePdfPage,
      },
    ],
  },
  {
    id: "jobs",
    label: copy.nav.groupJobs,
    sections: [
      {
        id: "jobs",
        ...s.jobs,
        icon: Briefcase,
        width: "wide",
        dirtyWorkspace: "job",
        component: JobsPage,
      },
      { id: "arbeitnow", ...s.arbeitnow, icon: Globe, width: "wide", component: ArbeitnowPage },
    ],
  },
  {
    id: "matching",
    label: copy.nav.groupMatching,
    sections: [
      { id: "matching", ...s.matching, icon: Scale, width: "wide", component: MatchingPage },
      {
        id: "reviews",
        ...s.reviews,
        icon: MessageSquareText,
        width: "wide",
        component: ReviewsPage,
      },
    ],
  },
  {
    id: "documents",
    label: copy.nav.groupDocuments,
    sections: [
      { id: "documents", ...s.documents, icon: FileText, width: "wide", component: DocumentsPage },
      { id: "artifacts", ...s.artifacts, icon: Files, width: "wide", component: ArtifactPage },
      {
        id: "generation",
        ...s.generation,
        icon: Sparkles,
        width: "form",
        component: GenerationPage,
      },
    ],
  },
  {
    id: "system",
    label: copy.nav.groupSystem,
    sections: [
      {
        id: "health",
        label: s.health.label,
        icon: Activity,
        width: "form",
        component: HealthPage,
      },
      { id: "settings", ...s.settings, icon: Settings, width: "form", component: SettingsPage },
    ],
  },
];

/** All sections flat, in navigation order (the Ctrl/Cmd+number shortcuts count in this order). */
export const SECTIONS: readonly SectionDefinition[] = SECTION_GROUPS.flatMap(
  (group) => group.sections,
);

/** The section for a `?section=` value; anything unknown is the default section. */
export function parseSection(value: unknown): SectionId {
  return SECTION_IDS.find((id) => id === value) ?? DEFAULT_SECTION;
}
