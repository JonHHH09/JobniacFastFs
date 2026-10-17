import { act, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { formatMessage } from "../i18n/messages";
import { CLOSE_REFUSED_EVENT, useCloseRefusal } from "./closeRefusal";

type Listener = (event: { payload: unknown }) => void;

// The listeners the hook registered, by event name. Hoisted so the mock factory can use it.
const listeners = vi.hoisted(() => new Map<string, Listener>());

vi.mock("@tauri-apps/api/event", () => ({
  listen: (name: string, listener: Listener) => {
    listeners.set(name, listener);
    return Promise.resolve(() => {
      listeners.delete(name);
    });
  },
}));

/** Shows the copy of the latest refused close, as the shell does. */
function Probe() {
  const { refusal, dismiss } = useCloseRefusal();
  return (
    <>
      <p data-testid="closeProbe">{refusal === null ? "" : formatMessage(refusal.id)}</p>
      <output data-testid="closeSerial">{refusal?.serial ?? 0}</output>
      <button type="button" onClick={dismiss}>
        dismiss
      </button>
    </>
  );
}

function refuse(messageId: string) {
  act(() => {
    listeners.get(CLOSE_REFUSED_EVENT)?.({
      payload: { workspace: "review", reason: "refuse_unsaved", messageId },
    });
  });
}

describe("refused window close", () => {
  it("shows the copy of the message id the Rust close gate sends", async () => {
    render(<Probe />);
    expect(screen.getByTestId("closeProbe").textContent).toBe("");

    await act(async () => {
      await Promise.resolve();
    });
    act(() => {
      listeners.get(CLOSE_REFUSED_EVENT)?.({
        payload: { workspace: "review", reason: "refuse_unsaved", messageId: "reviews.close" },
      });
    });

    expect(screen.getByTestId("closeProbe").textContent).toBe(
      "Finish the review request or discard the review draft before closing.",
    );
  });

  it("does not show the discard confirmation as a notice, the profile dialog carries it", async () => {
    render(<Probe />);
    await act(async () => {
      await Promise.resolve();
    });
    act(() => {
      listeners.get(CLOSE_REFUSED_EVENT)?.({
        payload: {
          workspace: "profile",
          reason: "confirm_discard",
          messageId: "profile.confirm.close",
        },
      });
    });

    expect(screen.getByTestId("closeProbe").textContent).toBe("");
  });

  it("ignores a message id the catalogue does not know", async () => {
    render(<Probe />);
    await act(async () => {
      await Promise.resolve();
    });
    act(() => {
      listeners.get(CLOSE_REFUSED_EVENT)?.({
        payload: { workspace: "match", reason: "busy", messageId: "not.a.message" },
      });
    });

    expect(screen.getByTestId("closeProbe").textContent).toBe("");
  });
});

describe("refused close notice lifecycle", () => {
  it("treats an identical repeated refusal as a new notice", async () => {
    render(<Probe />);
    await act(async () => {
      await Promise.resolve();
    });

    refuse("reviews.close");
    const first = screen.getByTestId("closeSerial").textContent;
    refuse("reviews.close");

    expect(screen.getByTestId("closeSerial").textContent).not.toBe(first);
  });

  it("clears the notice when it is dismissed", async () => {
    render(<Probe />);
    await act(async () => {
      await Promise.resolve();
    });
    refuse("reviews.close");
    expect(screen.getByTestId("closeProbe").textContent).not.toBe("");

    act(() => {
      screen.getByRole("button", { name: "dismiss" }).click();
    });

    expect(screen.getByTestId("closeProbe").textContent).toBe("");
  });
});
