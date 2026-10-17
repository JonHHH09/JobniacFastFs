import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useReducer, useRef, useState } from "react";
import type { MessageId } from "../../i18n/messages";
import { isMessageId } from "../../i18n/messages";
import type {
  PersonalDetailsEnvelopeDto,
  ProfileGraphDto,
  ProjectPatchDto,
} from "../../ipc/bindings";
import { isCommandError } from "../../ipc/commands";
import { deleteProfile, requestProfileDelete } from "../deletion/api";
import { confirmationMessage, deleteFailureId, isBusyRefusal } from "../deletion/confirmation";
import { useDeletionAvailable } from "../deletion/queries";
import { requestShellClose, setShellDirty } from "../shell/closeGate";
import {
  createProfile,
  fetchPersonalDetails,
  fetchProfile,
  updatePersonalDetails,
  updateProfile,
  updateProject,
} from "./api";
import {
  baselineProject,
  canSaveProject,
  isPersonalDirty,
  isProfileDirty,
  personalWriteRequest,
  projectPatchRequest,
  toWriteRequest,
} from "./draft";
import {
  type EditorAction,
  type EditorState,
  editorReducer,
  hasUnsavedChanges,
  initialEditorState,
  type PendingAction,
} from "./editorState";
import { forgetProfile, profileKeys, renameProfile } from "./queries";

/** The message id of a failed operation: the command's id when the catalogue has it. */
export function messageIdOf(error: unknown, fallback: MessageId): MessageId {
  if (isCommandError(error) && isMessageId(error.messageId)) {
    return error.messageId;
  }
  return fallback;
}

/**
 * True when Rust refused the operation because another one holds the guard. A refusal is not a
 * failure of the operation: the editor keeps its state and shows no save or load error.
 */
/** True when the saved graph passes the accept rules. */
export function savedGraphAccepted(
  graph: ProfileGraphDto,
  request: { mode: "new" | "edit"; profileId: string; revision: string },
): boolean {
  if (!/^\d+$/.test(graph.profile.revision) || graph.profile.id === "") {
    return false;
  }
  if (request.mode === "new") {
    return true;
  }
  return (
    graph.profile.id === request.profileId &&
    graph.profile.revision === nextRevision(request.revision)
  );
}

/** An ISO 8601 date-time as Rust writes it (RFC 3339 with `Z` or an offset). */
const TIMESTAMP_TEXT = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$/;

/** The revision after a decimal revision text, computed exactly; null when the text is not one. */
function nextRevision(revision: string): string | null {
  return /^\d+$/.test(revision) ? (BigInt(revision) + BigInt(1)).toString() : null;
}

/**
 * True when a saved personal envelope passes the presenter's accept rules: the same profile, the
 * loaded revision plus one, a timestamp, and personal details that name the same profile. Rust
 * validates the values; this only checks that the response belongs to the save that was sent.
 */
export function savedPersonalAccepted(
  envelope: PersonalDetailsEnvelopeDto,
  request: { profileId: string; revision: string },
): boolean {
  const details = envelope.personalDetails;
  return (
    envelope.profileId === request.profileId &&
    envelope.profileRevision === nextRevision(request.revision) &&
    TIMESTAMP_TEXT.test(envelope.profileUpdatedAt) &&
    details !== null &&
    details.profileId === envelope.profileId
  );
}

/**
 * True when a saved project passes the presenter's accept rules: the same project id and the
 * loaded revision plus one as the profile revision.
 */
export function savedProjectAccepted(
  result: ProjectPatchDto,
  request: { projectId: string; revision: string },
): boolean {
  return (
    result.project.id === request.projectId &&
    result.profileRevision === nextRevision(request.revision)
  );
}

/** What the editor screen can call. Every save and every confirmed action goes through here. */
export type ProfileEditorController = {
  state: EditorState;
  busy: boolean;
  loading: boolean;
  /** A profile, personal or project save is running (the save button shows the busy label). */
  saving: boolean;
  dirty: boolean;
  profileDirty: boolean;
  personalDirty: boolean;
  canSaveProfile: boolean;
  canEditPersonal: boolean;
  canSavePersonal: boolean;
  requestSelect: (profileId: string) => void;
  requestReload: () => void;
  requestNew: () => void;
  requestDiscard: () => void;
  /** The window close asked for the discard confirmation (Rust gate reason `confirm_discard`). */
  requestClose: () => void;
  confirmPending: () => void;
  cancelPending: () => void;
  saveProfile: () => void;
  savePersonal: () => void;
  saveProject: (index: number) => void;
  canSaveProjectRow: (index: number) => boolean;
  projectRowDirty: (index: number) => boolean;
  /** The delete control is shown: a profile is loaded and the delete use case is composed. */
  deleteVisible: boolean;
  canDeleteProfile: boolean;
  deleteConfirming: boolean;
  deleteMessage: string;
  deleting: boolean;
  requestDelete: () => void;
  confirmDelete: () => void;
  cancelDelete: () => void;
  dispatch: (action: EditorAction) => void;
};

/** A delete the user is asked to confirm: the Rust token, the id it names and the dialog text. */
type DeleteTarget = { profileId: string; token: string; message: string };

/**
 * The profile editor controller: the reducer, the TanStack mutations and the unsaved-changes flow.
 * Responses of an older load are dropped (the request counter).
 */
export function useProfileEditor(externalBusy = false): ProfileEditorController {
  const queryClient = useQueryClient();
  const [state, dispatch] = useReducer(editorReducer, initialEditorState);
  const [loading, setLoading] = useState(false);
  const latestLoad = useRef(0);
  // The latest state for callbacks that resolve later (a delete token that arrives after a switch).
  const stateRef = useRef(state);
  stateRef.current = state;

  const saveProfileMutation = useMutation({
    mutationFn: (input: {
      mode: "new" | "edit";
      profileId: string;
      revision: string;
      request: Record<string, unknown>;
    }) =>
      input.mode === "new"
        ? createProfile(input.request)
        : updateProfile(input.profileId, input.revision, input.request),
    onSuccess: (graph: ProfileGraphDto, input) => {
      if (!savedGraphAccepted(graph, input)) {
        dispatch({ type: "message", message: "profile.error.save" });
        return;
      }
      queryClient.setQueryData(profileKeys.graph(graph.profile.id), graph);
      dispatch(input.mode === "new" ? { type: "created", graph } : { type: "saved", graph });
      // The list query is manual (never active), so invalidation would not refetch it: the cached
      // rows are patched instead. A created profile shows in the list at the next Refresh.
      if (input.mode === "edit") {
        renameProfile(queryClient, graph.profile);
      }
    },
    onError: (error) => {
      if (!isBusyRefusal(error)) {
        dispatch({ type: "message", message: messageIdOf(error, "profile.error.save") });
      }
    },
  });

  const savePersonalMutation = useMutation({
    mutationFn: (input: {
      profileId: string;
      revision: string;
      request: Record<string, string | null>;
    }) => updatePersonalDetails(input.profileId, input.revision, input.request),
    onSuccess: (envelope, input) => {
      if (!savedPersonalAccepted(envelope, input)) {
        dispatch({ type: "message", message: "profile.error.save" });
        return;
      }
      queryClient.setQueryData(profileKeys.personal(envelope.profileId), envelope);
      dispatch({ type: "personalSaved", personal: envelope });
    },
    onError: (error) => {
      if (!isBusyRefusal(error)) {
        dispatch({ type: "message", message: messageIdOf(error, "profile.error.save") });
      }
    },
  });

  const saveProjectMutation = useMutation({
    mutationFn: (input: {
      projectId: string;
      revision: string;
      request: Record<string, unknown>;
    }) => updateProject(input.request),
    onSuccess: (result, input) => {
      if (!savedProjectAccepted(result, input)) {
        dispatch({ type: "message", message: "profile.error.project" });
        return;
      }
      dispatch({
        type: "projectSaved",
        project: result.project,
        profileRevision: result.profileRevision,
      });
    },
    onError: (error) => {
      if (!isBusyRefusal(error)) {
        dispatch({ type: "message", message: messageIdOf(error, "profile.error.project") });
      }
    },
  });

  const deleteAvailable = useDeletionAvailable(state.mode === "edit");
  const [deleteTarget, setDeleteTarget] = useState<DeleteTarget | null>(null);

  // Step one: Rust issues the token for the loaded profile. The dialog text is fixed at this point,
  // (the name and the unsaved state of the request).
  const requestDeleteMutation = useMutation({
    mutationFn: async (input: { profileId: string; message: string }): Promise<DeleteTarget> => {
      const confirmation = await requestProfileDelete(input.profileId);
      return {
        profileId: input.profileId,
        token: confirmation.confirmationToken,
        message: input.message,
      };
    },
    onSuccess: (target) => {
      // A token that arrives after the editor moved to another profile is dropped, as the job
      // store does: confirming it would act on a profile the user no longer looks at.
      const current = stateRef.current;
      if (current.mode === "edit" && current.profileId === target.profileId) {
        setDeleteTarget(target);
      }
    },
    onError: (error) => {
      const failure = deleteFailureId("profile", error);
      if (failure !== null) {
        dispatch({ type: "message", message: failure });
      }
    },
  });

  // Step two: the token is echoed back and consumed by Rust, whatever the outcome.
  const confirmDeleteMutation = useMutation({
    mutationFn: (target: DeleteTarget) => deleteProfile(target.profileId, target.token),
    onSuccess: (result, target) => {
      if (result.profileId !== target.profileId) {
        dispatch({ type: "message", message: "delete.failed.profile" });
        return;
      }
      // A profile that no longer exists is removed from the screen too (deleted: false).
      forgetProfile(queryClient, target.profileId);
      dispatch({
        type: "deleted",
        profileId: target.profileId,
        message: result.deleted ? "profile.deleted" : "profile.deleted.missing",
      });
    },
    onError: (error, target) => {
      // A busy refusal keeps the token (Rust) and the message: the same confirmation opens again.
      if (isBusyRefusal(error)) {
        setDeleteTarget(target);
        return;
      }
      const failure = deleteFailureId("profile", error);
      if (failure !== null) {
        dispatch({ type: "message", message: failure });
      }
    },
  });

  const saving =
    saveProfileMutation.isPending ||
    savePersonalMutation.isPending ||
    saveProjectMutation.isPending;
  const busy = loading || externalBusy || saving || confirmDeleteMutation.isPending;
  // The full-graph comparison runs once per draft change, not once per render.
  const profileDirty = useMemo(
    () =>
      state.mode === "edit" &&
      state.draft !== null &&
      state.baseline !== null &&
      isProfileDirty(state.draft, state.baseline),
    [state.mode, state.draft, state.baseline],
  );
  const personalDirty =
    state.mode === "edit" &&
    state.personal !== null &&
    state.personalBaseline !== null &&
    isPersonalDirty(state.personal, state.personalBaseline);
  // In edit mode the unsaved-changes rule is exactly profile or personal changed.
  const dirty = state.mode === "new" ? hasUnsavedChanges(state) : profileDirty || personalDirty;

  // A pending delete confirmation belongs to the profile it was requested for.
  useEffect(() => {
    if (deleteTarget !== null && deleteTarget.profileId !== state.profileId) {
      setDeleteTarget(null);
    }
  }, [deleteTarget, state.profileId]);

  // The dirty flag last reported to Rust; starts false, so nothing is sent until the draft changes.
  const reportedDirty = useRef(false);
  // The Rust close gate refuses while the draft holds unsaved edits, so each change is reported.
  useEffect(() => {
    if (reportedDirty.current === dirty) {
      return;
    }
    reportedDirty.current = dirty;
    void setShellDirty("profile", dirty).catch(() => undefined);
  }, [dirty]);
  // Unmounting drops the draft, so a page that unmounts while dirty must not keep the gate closed.
  useEffect(
    () => () => {
      if (reportedDirty.current) {
        reportedDirty.current = false;
        void setShellDirty("profile", false).catch(() => undefined);
      }
    },
    [],
  );

  async function loadProfile(profileId: string): Promise<void> {
    latestLoad.current += 1;
    const request = latestLoad.current;
    setLoading(true);
    // The previous status ("Profile loaded.") must not stay on screen while the new load runs.
    dispatch({ type: "message", message: null });
    try {
      // One after the other: both Rust commands take the same single-flight slot, so a concurrent
      // second call is refused as busy.
      const graph = await queryClient.fetchQuery({
        queryKey: profileKeys.graph(profileId),
        queryFn: () => fetchProfile(profileId),
      });
      const personal = await queryClient.fetchQuery({
        queryKey: profileKeys.personal(profileId),
        queryFn: () => fetchPersonalDetails(profileId),
      });
      if (request === latestLoad.current) {
        dispatch({ type: "loaded", graph, personal });
      }
    } catch (error) {
      if (request === latestLoad.current && !isBusyRefusal(error)) {
        dispatch({ type: "message", message: messageIdOf(error, "profile.error.load") });
      }
    } finally {
      if (request === latestLoad.current) {
        setLoading(false);
      }
    }
  }

  function runAction(pending: PendingAction): void {
    switch (pending.kind) {
      case "select":
        void loadProfile(pending.profileId);
        return;
      case "reload":
        if (state.profileId) {
          void loadProfile(state.profileId);
        }
        return;
      case "new":
        dispatch({ type: "newProfile" });
        return;
      case "discard":
        dispatch({ type: "discard" });
        return;
      case "close":
        // The edits are discarded by the confirmation, so Rust is told the draft is clean before the
        // gate runs again. The order matters: the clean report is awaited first.
        void setShellDirty("profile", false)
          .catch(() => undefined)
          .then(() => requestShellClose())
          .catch(() => undefined);
        return;
    }
  }

  /** Runs the action now when nothing would be lost; otherwise asks for the confirmation first. */
  function requestAction(pending: PendingAction): void {
    if (dirty) {
      dispatch({ type: "setPending", pending });
      return;
    }
    runAction(pending);
  }

  const canDeleteProfile =
    deleteAvailable &&
    state.mode === "edit" &&
    state.profileId !== null &&
    !busy &&
    !requestDeleteMutation.isPending &&
    deleteTarget === null &&
    state.pending === null;
  const deleteLabel = state.baseline?.fullName ?? "";

  return {
    state,
    busy,
    loading,
    saving,
    dirty,
    profileDirty,
    personalDirty,
    canSaveProfile:
      !busy &&
      state.draft !== null &&
      (state.mode === "new" || (state.mode === "edit" && profileDirty)),
    canEditPersonal: !busy && state.mode === "edit" && state.personal !== null,
    canSavePersonal: !busy && state.mode === "edit" && personalDirty,
    deleteVisible: deleteAvailable && state.mode === "edit",
    canDeleteProfile,
    deleteConfirming: deleteTarget !== null,
    deleteMessage: deleteTarget?.message ?? "",
    deleting: confirmDeleteMutation.isPending,
    requestDelete: () => {
      if (!canDeleteProfile || state.profileId === null) {
        return;
      }
      requestDeleteMutation.mutate({
        profileId: state.profileId,
        message: confirmationMessage("profile", deleteLabel, dirty),
      });
    },
    confirmDelete: () => {
      if (deleteTarget === null || confirmDeleteMutation.isPending) {
        return;
      }
      const target = deleteTarget;
      setDeleteTarget(null);
      confirmDeleteMutation.mutate(target);
    },
    cancelDelete: () => setDeleteTarget(null),
    dispatch,
    requestSelect: (profileId) => {
      // Selecting the loaded profile is a no-op while the editor is clean.
      if (state.mode === "edit" && state.profileId === profileId && !dirty) {
        return;
      }
      requestAction({ kind: "select", profileId, message: "profile.confirm.select" });
    },
    requestReload: () => requestAction({ kind: "reload", message: "profile.confirm.reload" }),
    requestNew: () => requestAction({ kind: "new", message: "profile.confirm.new" }),
    requestDiscard: () => requestAction({ kind: "discard", message: "profile.confirm.discard" }),
    requestClose: () => requestAction({ kind: "close", message: "profile.confirm.close" }),
    confirmPending: () => {
      const pending = state.pending;
      if (!pending) {
        return;
      }
      dispatch({ type: "setPending", pending: null });
      dispatch({ type: "discard" });
      runAction(pending);
    },
    cancelPending: () => dispatch({ type: "setPending", pending: null }),
    saveProfile: () => {
      const draft = state.draft;
      if (!draft || (state.mode === "edit" && !state.profileId)) {
        return;
      }
      saveProfileMutation.mutate({
        mode: state.mode === "new" ? "new" : "edit",
        profileId: state.profileId ?? "",
        revision: state.revision ?? "",
        request: toWriteRequest(draft),
      });
    },
    savePersonal: () => {
      const personal = state.personal;
      if (!personal || !state.profileId || !state.revision) {
        return;
      }
      savePersonalMutation.mutate({
        profileId: state.profileId,
        revision: state.revision,
        request: personalWriteRequest(personal),
      });
    },
    saveProject: (index) => {
      const projectId = state.draft?.collections.projects?.[index]?.id;
      if (
        !state.draft ||
        !state.baseline ||
        !state.profileId ||
        !state.revision ||
        typeof projectId !== "string"
      ) {
        return;
      }
      saveProjectMutation.mutate({
        projectId,
        revision: state.revision,
        request: projectPatchRequest(
          state.profileId,
          state.draft,
          state.baseline,
          index,
          state.revision,
        ),
      });
    },
    canSaveProjectRow: (index) =>
      !busy && state.mode === "edit" && canSaveProject(state.draft, state.baseline, index),
    projectRowDirty: (index) => {
      const row = state.draft?.collections.projects?.[index];
      const saved = baselineProject(state.baseline, row);
      return Boolean(row && saved && JSON.stringify(row) !== JSON.stringify(saved));
    },
  };
}
