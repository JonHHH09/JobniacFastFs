import { jobDetail, removeJob } from "./jobs";
import { profileGraph, removeProfile } from "./profiles";
import { type Args, fail, freshId, type HandlerMap } from "./util";

// A confirmation token is single use and bound to the row it was issued for.
const tokens = new Map<string, { kind: "profile" | "job"; id: string }>();

function issue(kind: "profile" | "job", id: unknown, exists: boolean) {
  if (!exists) {
    fail("not_found", `${kind}.error.notFound`);
  }
  const token = freshId();
  tokens.set(token, { kind, id: String(id) });
  return { confirmationToken: token };
}

function redeem(kind: "profile" | "job", id: unknown, token: unknown): void {
  const entry = tokens.get(String(token));
  tokens.delete(String(token));
  if (!entry || entry.kind !== kind || entry.id !== String(id)) {
    fail("validation_error", "deletion.error.token");
  }
}

export const deletionHandlers: HandlerMap = {
  deletion_capabilities_get: () => ({ deletion: true }),
  request_profile_delete: ({ profileId }: Args) =>
    issue("profile", profileId, profileGraph(String(profileId)) !== undefined),
  delete_profile: ({ profileId, confirmationToken }: Args) => {
    redeem("profile", profileId, confirmationToken);
    if (!removeProfile(String(profileId))) {
      fail("not_found", "profile.error.notFound");
    }
    return { profileId: String(profileId), deleted: true };
  },
  request_job_delete: ({ jobId }: Args) =>
    issue("job", jobId, jobDetail(String(jobId)) !== undefined),
  delete_job: ({ jobId, confirmationToken }: Args) => {
    redeem("job", jobId, confirmationToken);
    if (!removeJob(String(jobId))) {
      fail("not_found", "jobs.error.notFound");
    }
    return { jobId: String(jobId), deleted: true };
  },
};
