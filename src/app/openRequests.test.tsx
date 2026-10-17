import { act, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { requestOpen, resetOpenRequests, useOpenRequest } from "./openRequests";

const ID = "00000000-0000-4000-8000-000000000001";
const OTHER = "00000000-0000-4000-8000-000000000002";

function Section({ kind, onOpen }: { kind: "profile" | "job"; onOpen: (id: string) => void }) {
  useOpenRequest(kind, (request) => onOpen(request.id));
  return null;
}

afterEach(() => {
  resetOpenRequests();
});

describe("open requests", () => {
  it("delivers a request to a section that is already mounted", () => {
    const onOpen = vi.fn();
    render(<Section kind="profile" onOpen={onOpen} />);
    act(() => requestOpen({ kind: "profile", id: ID }));
    expect(onOpen).toHaveBeenCalledExactlyOnceWith(ID);
  });

  it("holds a request made before the section mounted and delivers it once it mounts", () => {
    const onOpen = vi.fn();
    requestOpen({ kind: "job", id: ID });
    expect(onOpen).not.toHaveBeenCalled();
    render(<Section kind="job" onOpen={onOpen} />);
    expect(onOpen).toHaveBeenCalledExactlyOnceWith(ID);
  });

  it("delivers a waiting request only once, even to a later section", () => {
    const first = vi.fn();
    const second = vi.fn();
    requestOpen({ kind: "job", id: ID });
    const view = render(<Section kind="job" onOpen={first} />);
    view.unmount();
    render(<Section kind="job" onOpen={second} />);
    expect(first).toHaveBeenCalledTimes(1);
    expect(second).not.toHaveBeenCalled();
  });

  it("keeps only the newest waiting request of a kind", () => {
    const onOpen = vi.fn();
    requestOpen({ kind: "profile", id: ID });
    requestOpen({ kind: "profile", id: OTHER });
    render(<Section kind="profile" onOpen={onOpen} />);
    expect(onOpen).toHaveBeenCalledExactlyOnceWith(OTHER);
  });

  it("keeps kinds apart", () => {
    const profile = vi.fn();
    const job = vi.fn();
    render(
      <>
        <Section kind="profile" onOpen={profile} />
        <Section kind="job" onOpen={job} />
      </>,
    );
    act(() => requestOpen({ kind: "job", id: ID }));
    expect(job).toHaveBeenCalledWith(ID);
    expect(profile).not.toHaveBeenCalled();
  });

  it("stops delivering after the section unmounts and waits for the next one", () => {
    const onOpen = vi.fn();
    const view = render(<Section kind="profile" onOpen={onOpen} />);
    view.unmount();
    requestOpen({ kind: "profile", id: ID });
    expect(onOpen).not.toHaveBeenCalled();
    const later = vi.fn();
    render(<Section kind="profile" onOpen={later} />);
    expect(later).toHaveBeenCalledExactlyOnceWith(ID);
  });

  it("calls the latest handler, not the one of the first render", () => {
    const first = vi.fn();
    const second = vi.fn();
    const view = render(<Section kind="profile" onOpen={first} />);
    view.rerender(<Section kind="profile" onOpen={second} />);
    act(() => requestOpen({ kind: "profile", id: ID }));
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledWith(ID);
  });
});
