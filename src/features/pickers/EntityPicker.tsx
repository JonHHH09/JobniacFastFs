import { X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { formatMessage } from "../../i18n/messages";
import { useDebounced } from "../../lib/useDebounced";
import { Button } from "../../ui/button";
import { Combobox, type ComboboxOption } from "../../ui/combobox";

/** The delay between the last key press and the search (UX wave 2). */
export const SEARCH_DEBOUNCE_MS = 200;

/** A canonical UUID. A pasted one is accepted as the id without a search (power users, tests). */
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function isUuid(text: string): boolean {
  return UUID_PATTERN.test(text.trim());
}

/** What a picker shows for one record. */
export type PickerEntity = { id: string; label: string; description?: string };

/** The props every picker takes. The value is the id string the screen keeps in its state. */
export type PickerProps = {
  /** The `id` of the text input, so the screen's `<label htmlFor>` and tests keep working. */
  id: string;
  testId?: string;
  /** The accessible name, when the screen has no `<label>` for the input. */
  label?: string;
  /** The selected id, or "" for none. A canonical UUID typed or pasted becomes the value as is. */
  value: string;
  onChange: (id: string) => void;
  disabled?: boolean;
  describedBy?: string;
  invalid?: boolean;
  placeholder?: string;
};

/** Where a picker gets its options and the display text of an id. */
export type EntitySource = {
  placeholder: string;
  emptyText: string;
  /** The empty text while the user has typed something that matches nothing. */
  noMatchText?: string;
  errorText: string;
  /** The options for a (debounced, trimmed) query; "" is the first page of the list. */
  useOptions: (
    query: string,
    enabled: boolean,
  ) => {
    entities: readonly PickerEntity[];
    fetching: boolean;
    failed: boolean;
    retry: () => void;
  };
  /** The record behind an id that was set from outside, to show its name instead of the id. */
  useResolved: (id: string, enabled: boolean) => PickerEntity | null;
};

/**
 * The shared picker: a combobox over an `EntitySource`. Typing searches (debounced); an empty query
 * lists the first page; choosing an option writes its id to `onChange`; a pasted canonical UUID is
 * written as it is. Editing the text after a choice clears the id until the next choice.
 */
export function EntityPicker({
  source,
  id,
  testId,
  label,
  value,
  onChange,
  disabled,
  describedBy,
  invalid,
  placeholder,
}: PickerProps & { source: EntitySource }) {
  const [text, setText] = useState("");
  const [typed, setTyped] = useState("");
  const [open, setOpen] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const known = useRef(new Map<string, PickerEntity>());
  const lastEmitted = useRef("");
  const rawTyped = useRef(false);

  const pastedId = isUuid(text);
  const query = useDebounced(typed.trim(), SEARCH_DEBOUNCE_MS);
  const options = source.useOptions(query, open && !pastedId && !disabled);
  const searching = typed.trim() !== query || options.fetching;
  const resolved = source.useResolved(
    value,
    value !== "" && isUuid(value) && !known.current.has(value),
  );

  const emit = (next: string) => {
    lastEmitted.current = next;
    onChange(next);
  };

  // A value set from outside (another part of the screen, a palette request) shows its name.
  useEffect(() => {
    if (value === lastEmitted.current) {
      return;
    }
    lastEmitted.current = value;
    rawTyped.current = false;
    setTyped("");
    if (value === "") {
      setText("");
      return;
    }
    setText(known.current.get(value)?.label ?? value);
  }, [value]);

  // The name arrives after the id: replace the raw id, unless the user typed that id themselves.
  useEffect(() => {
    if (resolved && resolved.id === value && !rawTyped.current && text === value) {
      known.current.set(resolved.id, resolved);
      setText(resolved.label);
    }
  }, [resolved, value, text]);

  const choose = (option: ComboboxOption) => {
    const entity = options.entities.find((candidate) => candidate.id === option.value);
    if (entity) {
      known.current.set(entity.id, entity);
      setText(entity.label);
    }
    rawTyped.current = false;
    setTyped("");
    emit(option.value);
  };

  const onInput = (next: string) => {
    setText(next);
    if (isUuid(next)) {
      rawTyped.current = true;
      setTyped("");
      emit(next.trim());
      return;
    }
    rawTyped.current = false;
    setTyped(next);
    if (value !== "" || lastEmitted.current !== "") {
      emit("");
    }
  };

  const clear = () => {
    setText("");
    setTyped("");
    rawTyped.current = false;
    if (value !== "") {
      emit("");
    }
    inputRef.current?.focus();
  };

  const comboOptions: ComboboxOption[] = pastedId
    ? []
    : options.entities.map((entity) => ({
        value: entity.id,
        label: entity.label,
        ...(entity.description ? { description: entity.description } : {}),
      }));

  return (
    <div className="min-w-0">
      <Combobox
        id={id}
        inputRef={inputRef}
        data-testid={testId}
        aria-label={label}
        aria-describedby={describedBy}
        aria-invalid={invalid ? true : undefined}
        disabled={disabled}
        placeholder={placeholder ?? source.placeholder}
        inputValue={text}
        onInputValueChange={onInput}
        options={comboOptions}
        onSelect={choose}
        onOpenChange={setOpen}
        autoActivateFirst={typed.trim() !== ""}
        loading={!pastedId && searching}
        emptyText={
          options.failed && !pastedId
            ? ""
            : pastedId
              ? ""
              : typed.trim() !== ""
                ? (source.noMatchText ?? source.emptyText)
                : source.emptyText
        }
        notice={
          options.failed && !pastedId ? (
            <div className="flex items-center justify-between gap-2 border-t border-border px-3 py-2">
              <p className="text-destructive">{source.errorText}</p>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onMouseDown={(event) => event.preventDefault()}
                onClick={options.retry}
              >
                {formatMessage("picker.retry")}
              </Button>
            </div>
          ) : undefined
        }
        endAdornment={
          text !== "" && !disabled ? (
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              aria-label={formatMessage("picker.clear")}
              title={formatMessage("picker.clear")}
              onMouseDown={(event) => event.preventDefault()}
              onClick={clear}
            >
              <X strokeWidth={1.75} aria-hidden />
            </Button>
          ) : undefined
        }
      />
      {pastedId ? (
        <p role="status" className="mt-1 text-[13px] text-muted-foreground">
          {formatMessage("picker.usingId")}
        </p>
      ) : null}
    </div>
  );
}
