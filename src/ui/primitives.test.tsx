import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Inbox } from "lucide-react";
import { describe, expect, it, vi } from "vitest";
import { EmptyState } from "./empty-state";
import { Field } from "./field";
import { Input } from "./input";
import { ListRow } from "./list-row";
import { PageHeader } from "./page-header";
import { ScoreMeter } from "./score-meter";
import { SegmentedControl } from "./segmented-control";
import { Select } from "./select";
import { StatusMessage } from "./status-message";
import { Switch } from "./switch";

describe("ui primitives", () => {
  it("score meter exposes its value as text and clamps it to 0..100", () => {
    const { rerender } = render(<ScoreMeter value={72} label="Match score" />);
    const meter = screen.getByRole("meter", { name: "Match score" });
    expect(meter).toHaveAttribute("aria-valuenow", "72");
    expect(meter).toHaveAttribute("aria-valuemin", "0");
    expect(meter).toHaveAttribute("aria-valuemax", "100");
    expect(meter).toHaveAttribute("aria-valuetext", "72 of 100");
    rerender(<ScoreMeter value={140} label="Match score" />);
    expect(screen.getByRole("meter")).toHaveAttribute("aria-valuenow", "100");
  });

  it("status message is a polite live region whose error tone carries text", () => {
    render(<StatusMessage tone="error">The profile could not be saved.</StatusMessage>);
    const status = screen.getByRole("status");
    expect(status).toHaveAttribute("aria-live", "polite");
    expect(status).toHaveTextContent("Error: The profile could not be saved.");
  });

  it("empty state shows its title, text and action", () => {
    render(
      <EmptyState
        icon={Inbox}
        title="No profiles yet"
        text="Create one."
        action={<button type="button">New</button>}
      />,
    );
    expect(screen.getByText("No profiles yet")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "New" })).toBeInTheDocument();
  });

  it("page header renders the one h1 with its description and actions", () => {
    render(
      <PageHeader
        title="Profiles"
        description="Manage profiles."
        actions={<button type="button">Add</button>}
      />,
    );
    expect(screen.getByRole("heading", { level: 1, name: "Profiles" })).toBeInTheDocument();
    expect(screen.getByText("Manage profiles.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add" })).toBeInTheDocument();
  });
});

describe("form and list primitives", () => {
  it("field wires label, hint and error to the control", () => {
    render(
      <Field label="Email" hint="Work address" error="Enter a valid address">
        {(control) => <Input {...control} />}
      </Field>,
    );
    const input = screen.getByLabelText("Email");
    expect(input).toHaveAttribute("aria-invalid", "true");
    const ids = (input.getAttribute("aria-describedby") ?? "").split(" ");
    expect(ids).toHaveLength(2);
    expect(document.getElementById(ids[0] ?? "")).toHaveTextContent("Work address");
    expect(document.getElementById(ids[1] ?? "")).toHaveTextContent("Enter a valid address");
  });

  it("field without hint or error adds no aria attributes", () => {
    render(<Field label="Name">{(control) => <Input {...control} />}</Field>);
    const input = screen.getByLabelText("Name");
    expect(input).not.toHaveAttribute("aria-invalid");
    expect(input).not.toHaveAttribute("aria-describedby");
  });

  it("list row marks the selected row and activates from the keyboard", async () => {
    const onSelect = vi.fn();
    render(
      <>
        <ListRow title="Backend engineer" meta="Remote" selected onSelect={onSelect} />
        <ListRow title="Static row" />
      </>,
    );
    const row = screen.getByRole("button", { name: /Backend engineer/ });
    expect(row).toHaveAttribute("aria-current", "true");
    row.focus();
    await userEvent.keyboard("{Enter}");
    await userEvent.keyboard(" ");
    expect(onSelect).toHaveBeenCalledTimes(2);
    const plain = screen.getByText("Static row").closest("[data-slot=list-row]");
    expect(plain?.tagName).toBe("DIV");
    expect(plain).not.toHaveAttribute("aria-current");
  });

  it("segmented control moves the choice with the arrow keys", async () => {
    const onValueChange = vi.fn();
    render(
      <SegmentedControl
        aria-label="Zoom"
        value="fit"
        onValueChange={onValueChange}
        options={[
          { value: "fit", label: "Fit" },
          { value: "100", label: "100%" },
        ]}
      />,
    );
    await userEvent.click(screen.getByRole("radio", { name: "Fit" }));
    await userEvent.keyboard("{ArrowRight}");
    expect(screen.getByRole("radio", { name: "100%" })).toHaveFocus();
    await userEvent.keyboard(" ");
    await waitFor(() => expect(onValueChange).toHaveBeenCalledWith("100"));
  });

  it("switch is named by its label, toggles from the keyboard and can be disabled", async () => {
    const onCheckedChange = vi.fn();
    const { rerender } = render(
      <>
        <label htmlFor="power">Power</label>
        <Switch id="power" checked={false} onCheckedChange={onCheckedChange} />
      </>,
    );
    const toggle = screen.getByRole("switch", { name: "Power" });
    expect(toggle).not.toBeChecked();
    toggle.focus();
    await userEvent.keyboard(" ");
    expect(onCheckedChange).toHaveBeenCalledWith(true);
    rerender(
      <>
        <label htmlFor="power">Power</label>
        <Switch id="power" checked disabled onCheckedChange={onCheckedChange} />
      </>,
    );
    expect(screen.getByRole("switch", { name: "Power" })).toBeChecked();
    expect(screen.getByRole("switch", { name: "Power" })).toBeDisabled();
  });

  it("select renders a native select with its options", () => {
    render(
      <Select aria-label="Language" defaultValue="de">
        <option value="en">English</option>
        <option value="de">Deutsch</option>
      </Select>,
    );
    expect(screen.getByRole("combobox", { name: "Language" })).toHaveValue("de");
  });
});
