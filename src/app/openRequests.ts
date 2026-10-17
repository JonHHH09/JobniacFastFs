import { useEffect, useRef } from "react";

/** What a section is asked to open: a saved profile or a saved job, by canonical id. */
export type OpenRequest = { kind: "profile" | "job"; id: string };

export type OpenRequestKind = OpenRequest["kind"];

type Handler = (request: OpenRequest) => void;

const handlers: Record<OpenRequestKind, Set<Handler>> = {
  profile: new Set(),
  job: new Set(),
};

// A request made while no section listens (its section has not mounted yet) waits here, one per
// kind: the newest wins, because only the last thing the user asked for matters.
const pending: Partial<Record<OpenRequestKind, OpenRequest>> = {};

function deliver(request: OpenRequest, targets: Iterable<Handler>): void {
  for (const handler of targets) {
    handler(request);
  }
}

/**
 * Asks the section that owns `kind` to open the entity. A mounted subscriber gets it at once; if the
 * section has not mounted yet (sections mount on their first visit), the request is kept and
 * delivered to the first subscriber that mounts.
 */
export function requestOpen(request: OpenRequest): void {
  const subscribed = handlers[request.kind];
  if (subscribed.size === 0) {
    pending[request.kind] = request;
    return;
  }
  delete pending[request.kind];
  deliver(request, [...subscribed]);
}

/** Subscribes to one kind outside React; returns the unsubscribe function. */
export function subscribeOpenRequests(kind: OpenRequestKind, handler: Handler): () => void {
  handlers[kind].add(handler);
  const waiting = pending[kind];
  if (waiting) {
    delete pending[kind];
    handler(waiting);
  }
  return () => {
    handlers[kind].delete(handler);
  };
}

/**
 * A section calls this to open what the palette (or any other part of the shell) asked for. The
 * handler may change between renders; the subscription lives as long as the component.
 */
export function useOpenRequest(kind: OpenRequestKind, handler: Handler): void {
  const latest = useRef(handler);
  latest.current = handler;
  useEffect(() => subscribeOpenRequests(kind, (request) => latest.current(request)), [kind]);
}

/** Drops every subscriber and pending request. For tests. */
export function resetOpenRequests(): void {
  handlers.profile.clear();
  handlers.job.clear();
  delete pending.profile;
  delete pending.job;
}
