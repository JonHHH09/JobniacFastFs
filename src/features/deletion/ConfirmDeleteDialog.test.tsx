import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ConfirmDeleteDialog } from "./ConfirmDeleteDialog";

const MESSAGE = 'Delete profile "Ada Example"? This cannot be undone.';

function renderDialog(confirming = true, kind: "profile" | "job" = "profile") {
  const onConfirm = vi.fn();
  const onCancel = vi.fn();
  render(
    <ConfirmDeleteDialog
      kind={kind}
      confirming={confirming}
      message={MESSAGE}
      onConfirm={onConfirm}
      onCancel={onCancel}
    />,
  );
  return { onConfirm, onCancel };
}

describe("ConfirmDeleteDialog", () => {
  it("renders nothing while it is not confirming", () => {
    renderDialog(false);
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });

  it("is an alert dialog titled for the profile, with the message as plain text", () => {
    renderDialog();
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
    expect(screen.getByText("Delete this profile permanently?")).toBeInTheDocument();
    expect(screen.getByTestId("deleteProfileDialogMessage")).toHaveTextContent(MESSAGE);
  });

  it("does not render markup from the message", () => {
    const onConfirm = vi.fn();
    render(
      <ConfirmDeleteDialog
        kind="job"
        confirming
        message="<b>Synthetic</b> job"
        onConfirm={onConfirm}
        onCancel={vi.fn()}
      />,
    );
    const message = screen.getByTestId("deleteJobDialogMessage");
    expect(message.textContent).toBe("<b>Synthetic</b> job");
    expect(message.querySelector("b")).toBeNull();
  });

  it("focuses Cancel when it opens", async () => {
    renderDialog();
    await waitFor(() =>
      expect(screen.getByTestId("deleteProfileDialogCancelButton")).toHaveFocus(),
    );
  });

  it("uses the job test ids and title for a job", () => {
    renderDialog(true, "job");
    expect(screen.getByText("Delete this job permanently?")).toBeInTheDocument();
    expect(screen.getByTestId("deleteJobDialogConfirmButton")).toBeInTheDocument();
  });

  it("gives Cancel and the confirm button their accessible names", () => {
    renderDialog();
    expect(screen.getByRole("button", { name: "Cancel and keep everything" })).toBe(
      screen.getByTestId("deleteProfileDialogCancelButton"),
    );
    expect(screen.getByRole("button", { name: "Delete permanently" })).toBe(
      screen.getByTestId("deleteProfileDialogConfirmButton"),
    );
  });

  it("Escape cancels and does not confirm", () => {
    const { onConfirm, onCancel } = renderDialog();
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("an outside click does not dismiss the dialog", () => {
    const { onCancel } = renderDialog();
    fireEvent.pointerDown(document.body, { button: 0, pointerType: "mouse" });
    fireEvent.mouseDown(document.body, { button: 0 });
    fireEvent.click(document.body);
    expect(onCancel).not.toHaveBeenCalled();
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
  });

  it("Cancel runs the cancel path only", () => {
    const { onConfirm, onCancel } = renderDialog();
    fireEvent.click(screen.getByTestId("deleteProfileDialogCancelButton"));
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("the confirm button runs the confirm path only", () => {
    const { onConfirm, onCancel } = renderDialog();
    fireEvent.click(screen.getByTestId("deleteProfileDialogConfirmButton"));
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(onCancel).not.toHaveBeenCalled();
  });
});
