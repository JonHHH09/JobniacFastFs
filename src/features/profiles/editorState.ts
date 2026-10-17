import type { MessageId } from "../../i18n/messages";
import type { PersonalDetailsEnvelopeDto, ProfileGraphDto, ProjectDto } from "../../ipc/bindings";
import {
  addRow,
  addTechnology,
  blankDraft,
  blankPersonal,
  draftFromGraph,
  isPersonalDirty,
  isProfileDirty,
  type PersonalDraft,
  type ProfileDraft,
  personalFromEnvelope,
  removeRow,
  removeTechnology,
  setRowField,
  setTechnology,
} from "./draft";
import type { CollectionKey } from "./schema";

/**
 * The profile editor state (UI state only). `none` shows no profile,
 * `new` edits a blank draft, `edit` edits a loaded profile against its baseline.
 */
export type EditorMode = "none" | "new" | "edit";

/** An action that waits for the unsaved-changes confirmation. */
export type PendingAction =
  | { kind: "select"; profileId: string; message: MessageId }
  | { kind: "reload"; message: MessageId }
  | { kind: "new"; message: MessageId }
  | { kind: "discard"; message: MessageId }
  | { kind: "close"; message: MessageId };

/** The editor state. `version` changes whenever the baseline is replaced (it remounts the form). */
export type EditorState = {
  mode: EditorMode;
  profileId: string | null;
  revision: string | null;
  draft: ProfileDraft | null;
  baseline: ProfileDraft | null;
  personal: PersonalDraft | null;
  personalBaseline: PersonalDraft | null;
  hasPhoto: boolean;
  version: number;
  message: MessageId | null;
  pending: PendingAction | null;
};

export const initialEditorState: EditorState = {
  mode: "none",
  profileId: null,
  revision: null,
  draft: null,
  baseline: null,
  personal: null,
  personalBaseline: null,
  hasPhoto: false,
  version: 0,
  message: null,
  pending: null,
};

/** Every change of the editor. */
export type EditorAction =
  | { type: "loaded"; graph: ProfileGraphDto; personal: PersonalDetailsEnvelopeDto }
  | { type: "created"; graph: ProfileGraphDto }
  | { type: "saved"; graph: ProfileGraphDto }
  | { type: "personalSaved"; personal: PersonalDetailsEnvelopeDto }
  | { type: "projectSaved"; project: ProjectDto; profileRevision: string }
  | { type: "newProfile" }
  | { type: "discard" }
  | { type: "setField"; field: "fullName" | "email" | "summary"; value: string }
  | { type: "setRowField"; collection: CollectionKey; index: number; field: string; value: string }
  | { type: "addRow"; collection: CollectionKey }
  | { type: "removeRow"; collection: CollectionKey; index: number }
  | { type: "addTechnology"; projectIndex: number }
  | { type: "removeTechnology"; projectIndex: number; technologyIndex: number }
  | { type: "setTechnology"; projectIndex: number; technologyIndex: number; value: string }
  | { type: "setPersonal"; field: keyof PersonalDraft; value: string }
  | { type: "message"; message: MessageId | null }
  | { type: "setPending"; pending: PendingAction | null }
  | { type: "deleted"; profileId: string; message: MessageId };

/** Whether the editor holds edits the user could lose. */
export function hasUnsavedChanges(state: EditorState): boolean {
  if (state.mode === "new") {
    const draft = state.draft;
    if (!draft) {
      return false;
    }
    return (
      draft.fullName !== "" ||
      draft.email !== "" ||
      draft.summary !== "" ||
      Object.values(draft.collections).some((rows) => rows !== undefined && rows.length > 0)
    );
  }
  if (state.mode !== "edit") {
    return false;
  }
  const profileChanged =
    state.draft !== null && state.baseline !== null && isProfileDirty(state.draft, state.baseline);
  const personalChanged =
    state.personal !== null &&
    state.personalBaseline !== null &&
    isPersonalDirty(state.personal, state.personalBaseline);
  return profileChanged || personalChanged;
}

/** The profile draft is dirty against its baseline (used for the discard action). */
export function isEditDirty(state: EditorState): boolean {
  return (
    state.mode === "edit" &&
    state.draft !== null &&
    state.baseline !== null &&
    isProfileDirty(state.draft, state.baseline)
  );
}

/** Loads a graph and its personal envelope, or reports a mismatch between the two (Rust rule). */
export function loadedState(
  graph: ProfileGraphDto,
  personal: PersonalDetailsEnvelopeDto,
  version: number,
): EditorState {
  const consistent =
    graph.profile.id === personal.profileId && graph.profile.revision === personal.profileRevision;
  if (!consistent) {
    return { ...initialEditorState, version, message: "profile.error.conflict" };
  }
  const draft = draftFromGraph(graph);
  const personalDraft = personalFromEnvelope(personal);
  return {
    mode: "edit",
    profileId: graph.profile.id,
    revision: graph.profile.revision,
    draft,
    baseline: draft,
    personal: personalDraft,
    personalBaseline: personalDraft,
    hasPhoto: Boolean(personal.personalDetails?.photoDocumentId),
    version: version + 1,
    message: "profile.loaded",
    pending: null,
  };
}

function editDraft(state: EditorState, edit: (draft: ProfileDraft) => ProfileDraft): EditorState {
  if (!state.draft) {
    return state;
  }
  return { ...state, draft: edit(state.draft), message: null };
}

/** Pure reducer: every transition of the editor. Unchanged values return the same state. */
export function editorReducer(state: EditorState, action: EditorAction): EditorState {
  switch (action.type) {
    case "loaded":
      return loadedState(action.graph, action.personal, state.version);
    case "created": {
      const draft = draftFromGraph(action.graph);
      return {
        ...state,
        mode: "edit",
        profileId: action.graph.profile.id,
        revision: action.graph.profile.revision,
        draft,
        baseline: draft,
        personal: blankPersonal,
        personalBaseline: blankPersonal,
        hasPhoto: false,
        version: state.version + 1,
        message: "profile.created",
        pending: null,
      };
    }
    case "saved": {
      const draft = draftFromGraph(action.graph);
      return {
        ...state,
        revision: action.graph.profile.revision,
        draft,
        baseline: draft,
        version: state.version + 1,
        message: "profile.saved",
      };
    }
    case "personalSaved": {
      const personal = personalFromEnvelope(action.personal);
      return {
        ...state,
        revision: action.personal.profileRevision,
        personal,
        personalBaseline: personal,
        hasPhoto: Boolean(action.personal.personalDetails?.photoDocumentId),
        version: state.version + 1,
        message: "personal.saved",
      };
    }
    case "projectSaved": {
      if (!state.draft || !state.baseline) {
        return state;
      }
      // The presenter copies only these four fields; the row's technologies stay as they are.
      const saved = {
        name: action.project.name,
        url: action.project.url,
        description: action.project.description,
        displayOrder: action.project.displayOrder,
      };
      const replaceAt = (rows: NonNullable<ProfileDraft["collections"]["projects"]>) =>
        rows.map((row) => (row.id === action.project.id ? { ...row, ...saved } : row));
      const baseline: ProfileDraft = {
        ...state.baseline,
        collections: {
          ...state.baseline.collections,
          projects: replaceAt(state.baseline.collections.projects ?? []),
        },
      };
      const draft: ProfileDraft = {
        ...state.draft,
        collections: {
          ...state.draft.collections,
          projects: replaceAt(state.draft.collections.projects ?? []),
        },
      };
      const stillDirty = isProfileDirty(draft, baseline);
      return {
        ...state,
        revision: action.profileRevision,
        draft,
        baseline,
        message: stillDirty ? "profile.project.saved.dirty" : "profile.project.saved",
      };
    }
    case "newProfile":
      return {
        ...initialEditorState,
        mode: "new",
        draft: blankDraft(),
        version: state.version + 1,
        message: "profile.new.hint",
      };
    case "discard":
      if (state.mode === "new") {
        // A new draft has no baseline: discarding it returns to the blank form.
        return {
          ...initialEditorState,
          mode: "new",
          draft: blankDraft(),
          version: state.version + 1,
          message: "profile.discarded",
        };
      }
      if (!state.baseline) {
        return state;
      }
      return {
        ...state,
        draft: state.baseline,
        personal: state.personalBaseline,
        version: state.version + 1,
        message: "profile.discarded",
        pending: null,
      };
    case "setField":
      return editDraft(state, (draft) => ({ ...draft, [action.field]: action.value }));
    case "setRowField":
      return editDraft(state, (draft) =>
        setRowField(draft, action.collection, action.index, action.field, action.value),
      );
    case "addRow": {
      if (!state.draft) {
        return state;
      }
      const result = addRow(state.draft, action.collection);
      return result.ok
        ? { ...state, draft: result.draft, message: null }
        : { ...state, message: result.messageId };
    }
    case "removeRow":
      return editDraft(state, (draft) => removeRow(draft, action.collection, action.index));
    case "addTechnology": {
      if (!state.draft) {
        return state;
      }
      const result = addTechnology(state.draft, action.projectIndex);
      return result.ok
        ? { ...state, draft: result.draft, message: null }
        : { ...state, message: result.messageId };
    }
    case "removeTechnology":
      return editDraft(state, (draft) =>
        removeTechnology(draft, action.projectIndex, action.technologyIndex),
      );
    case "setTechnology":
      return editDraft(state, (draft) =>
        setTechnology(draft, action.projectIndex, action.technologyIndex, action.value),
      );
    case "setPersonal": {
      if (!state.personal || state.personal[action.field] === action.value) {
        return state;
      }
      return { ...state, personal: { ...state.personal, [action.field]: action.value } };
    }
    case "message":
      return { ...state, message: action.message };
    case "setPending":
      return { ...state, pending: action.pending };
    case "deleted":
      // A late confirmation for a profile the editor no longer shows leaves the editor alone.
      if (state.profileId !== action.profileId) {
        return { ...state, message: action.message };
      }
      // The loaded profile no longer exists: the editor returns to `none` and keeps the shown message.
      return { ...initialEditorState, version: state.version + 1, message: action.message };
  }
}
