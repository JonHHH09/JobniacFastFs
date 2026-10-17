import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { Combobox, type ComboboxOption } from "./combobox";

const OPTIONS: ComboboxOption[] = [
  { value: "a", label: "Ada Lovelace", description: "ada@example.com" },
  { value: "b", label: "Grace Hopper", description: "grace@example.com" },
  { value: "c", label: "Alan Turing", description: "alan@example.com" },
];

function Harness({
  options = OPTIONS,
  loading = false,
  onSelect = () => undefined,
  autoActivateFirst = false,
  inline = false,
}: {
  options?: ComboboxOption[];
  loading?: boolean;
  onSelect?: (option: ComboboxOption) => void;
  autoActivateFirst?: boolean;
  inline?: boolean;
}) {
  const [text, setText] = useState("");
  return (
    <Combobox
      id="people"
      aria-label="Person"
      inputValue={text}
      onInputValueChange={setText}
      options={options}
      loading={loading}
      autoActivateFirst={autoActivateFirst}
      inline={inline}
      onSelect={(option) => {
        setText(option.label);
        onSelect(option);
      }}
    />
  );
}

function input(): HTMLElement {
  return screen.getByRole("combobox", { name: "Person" });
}

describe("combobox", () => {
  it("is closed until the input is focused, then lists the options", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    expect(input()).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    await user.click(input());
    expect(input()).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("listbox", { name: "Person" })).toBeInTheDocument();
    expect(screen.getAllByRole("option")).toHaveLength(3);
    expect(screen.getByRole("status")).toHaveTextContent("3 results available.");
  });

  it("moves with Down and Up, wraps, and names the active option", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(input());
    expect(input()).not.toHaveAttribute("aria-activedescendant");
    await user.keyboard("{ArrowDown}");
    const options = screen.getAllByRole("option");
    expect(input()).toHaveAttribute("aria-activedescendant", options[0]?.id);
    expect(options[0]).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(input()).toHaveAttribute("aria-activedescendant", options[2]?.id);
    await user.keyboard("{ArrowDown}");
    expect(input()).toHaveAttribute("aria-activedescendant", options[0]?.id);
    await user.keyboard("{ArrowUp}");
    expect(input()).toHaveAttribute("aria-activedescendant", options[2]?.id);
    expect(input()).toHaveFocus();
  });

  it("jumps with Home and End while an option is active", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(input());
    await user.keyboard("{ArrowDown}{End}");
    const options = screen.getAllByRole("option");
    expect(input()).toHaveAttribute("aria-activedescendant", options[2]?.id);
    await user.keyboard("{Home}");
    expect(input()).toHaveAttribute("aria-activedescendant", options[0]?.id);
  });

  it("leaves Home and End to the text caret when no option is active", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(input());
    await user.keyboard("{Home}{End}");
    expect(input()).not.toHaveAttribute("aria-activedescendant");
  });

  it("opens with Down when closed and selects the highlighted option with Enter", async () => {
    const onSelect = vi.fn();
    const user = userEvent.setup();
    render(<Harness onSelect={onSelect} />);
    await user.click(input());
    await user.keyboard("{Escape}");
    expect(input()).toHaveAttribute("aria-expanded", "false");
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(input()).toHaveAttribute("aria-expanded", "true");
    await user.keyboard("{Enter}");
    expect(onSelect).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ value: "b" }));
    expect(input()).toHaveValue("Grace Hopper");
    expect(input()).toHaveAttribute("aria-expanded", "false");
    expect(input()).toHaveFocus();
  });

  it("does not take Enter when no option is active, so a form can submit", async () => {
    const onSelect = vi.fn();
    const onSubmit = vi.fn((event: React.FormEvent) => event.preventDefault());
    const user = userEvent.setup();
    render(
      <form onSubmit={onSubmit}>
        <Harness onSelect={onSelect} />
      </form>,
    );
    await user.click(input());
    await user.keyboard("{Enter}");
    expect(onSelect).not.toHaveBeenCalled();
    expect(onSubmit).toHaveBeenCalledTimes(1);
  });

  it("highlights the first option at once with autoActivateFirst", async () => {
    const onSelect = vi.fn();
    const user = userEvent.setup();
    render(<Harness autoActivateFirst onSelect={onSelect} />);
    await user.click(input());
    expect(screen.getAllByRole("option")[0]).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{Enter}");
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ value: "a" }));
  });

  it("closes with Escape and keeps the key from reaching a dialog around it", async () => {
    const outer = vi.fn();
    const user = userEvent.setup();
    render(
      // biome-ignore lint/a11y/noStaticElementInteractions: probe for event propagation
      <div onKeyDown={outer}>
        <Harness />
      </div>,
    );
    await user.click(input());
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(outer).not.toHaveBeenCalledWith(expect.objectContaining({ key: "Escape" }));
  });

  it("selects with a click and keeps focus in the input", async () => {
    const onSelect = vi.fn();
    const user = userEvent.setup();
    render(<Harness onSelect={onSelect} />);
    await user.click(input());
    await user.click(screen.getByRole("option", { name: /Alan Turing/ }));
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ value: "c" }));
    expect(input()).toHaveFocus();
  });

  it("closes when focus leaves with Tab", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(input());
    await user.tab();
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("shows a loading row and an empty row as announced status text", async () => {
    const user = userEvent.setup();
    const { rerender } = render(<Harness options={[]} loading />);
    await user.click(input());
    expect(screen.getByRole("status")).toHaveTextContent("Searching…");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    rerender(<Harness options={[]} />);
    expect(screen.getByRole("status")).toHaveTextContent("No results.");
    expect(input()).not.toHaveAttribute("aria-controls");
  });

  it("lists grouped options under their headings", async () => {
    const user = userEvent.setup();
    render(
      <Harness
        options={[
          { value: "s1", label: "Profiles", group: "Sections" },
          { value: "p1", label: "Ada Lovelace", group: "People" },
        ]}
      />,
    );
    await user.click(input());
    expect(screen.getByText("Sections")).toBeInTheDocument();
    expect(screen.getByText("People")).toBeInTheDocument();
    expect(screen.getAllByRole("option")).toHaveLength(2);
  });

  it("renders inline lists open and leaves Escape to the dialog around it", async () => {
    const outer = vi.fn();
    const user = userEvent.setup();
    render(
      // biome-ignore lint/a11y/noStaticElementInteractions: probe for event propagation
      <div onKeyDown={outer}>
        <Harness inline />
      </div>,
    );
    expect(screen.getByRole("listbox")).toBeInTheDocument();
    await user.click(input());
    await user.keyboard("{Escape}");
    expect(screen.getByRole("listbox")).toBeInTheDocument();
    expect(outer).toHaveBeenCalledWith(expect.objectContaining({ key: "Escape" }));
  });
});
