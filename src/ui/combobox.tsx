import * as React from "react";
import { createPortal } from "react-dom";
import { formatMessage } from "@/i18n/messages";
import { cn } from "@/lib/utils";
import { Input } from "./input";
import { Spinner } from "./spinner";

type ComboboxOption = {
  /** The value handed to `onSelect`; unique within the list. */
  value: string;
  /** The primary text (14px/600). */
  label: string;
  /** A second line in graphite. */
  description?: string;
  icon?: React.ReactNode;
  trailing?: React.ReactNode;
  /** Options with the same `group` are listed under one heading, in the order they first appear. */
  group?: string;
  disabled?: boolean;
};

type ComboboxProps = {
  id: string;
  /** The text of the input. The caller owns it and decides what typing means (usually a search). */
  inputValue: string;
  onInputValueChange: (value: string) => void;
  /** The options to show, already filtered: the combobox does not filter (options are async). */
  options: readonly ComboboxOption[];
  onSelect: (option: ComboboxOption) => void;
  loading?: boolean;
  /** The row shown when `options` is empty and nothing loads. */
  emptyText?: string;
  loadingText?: string;
  /** Extra content under the options inside the popup (an error with a retry button, a hint). */
  notice?: React.ReactNode;
  /** Highlights the first option whenever the list changes, so Enter takes it. */
  autoActivateFirst?: boolean;
  /** Keeps the popup closed until the user types or presses Down (default: opens on focus). */
  openOnFocus?: boolean;
  /** Renders the list in the page flow, always open, instead of in a floating popup (the palette). */
  inline?: boolean;
  /** Content inside the right edge of the input (a clear button, a spinner). */
  endAdornment?: React.ReactNode;
  disabled?: boolean;
  placeholder?: string;
  autoFocus?: boolean;
  name?: string;
  className?: string;
  inputClassName?: string;
  inputRef?: React.Ref<HTMLInputElement>;
  /** Called first for every key press in the input; call `preventDefault` to take the key over. */
  onInputKeyDown?: (event: React.KeyboardEvent<HTMLInputElement>) => void;
  onFocus?: React.FocusEventHandler<HTMLInputElement>;
  onBlur?: React.FocusEventHandler<HTMLInputElement>;
  onOpenChange?: (open: boolean) => void;
  /** The accessible name of the list; defaults to the input's `aria-label`. */
  listLabel?: string;
  "aria-label"?: string;
  "aria-labelledby"?: string;
  "aria-describedby"?: string;
  "aria-invalid"?: boolean | "true" | "false";
  "data-testid"?: string;
};

type PopupBox = {
  left: number;
  width: number;
  top?: number;
  bottom?: number;
  maxHeight: number;
};

const POPUP_MAX = 320;
const GAP = 4;

/** Measures the anchor so a fixed popup can sit under (or, when short of room, over) it. */
function usePopupBox(
  anchor: React.RefObject<HTMLElement | null>,
  active: boolean,
): PopupBox | null {
  const [box, setBox] = React.useState<PopupBox | null>(null);
  React.useLayoutEffect(() => {
    if (!active) {
      setBox(null);
      return;
    }
    const measure = () => {
      const el = anchor.current;
      if (!el) {
        return;
      }
      const rect = el.getBoundingClientRect();
      const below = window.innerHeight - rect.bottom - GAP - 8;
      const above = rect.top - GAP - 8;
      const flip = below < 200 && above > below;
      const next: PopupBox = flip
        ? {
            left: rect.left,
            width: rect.width,
            bottom: window.innerHeight - rect.top + GAP,
            maxHeight: Math.max(120, Math.min(POPUP_MAX, above)),
          }
        : {
            left: rect.left,
            width: rect.width,
            top: rect.bottom + GAP,
            maxHeight: Math.max(120, Math.min(POPUP_MAX, below)),
          };
      setBox((previous) =>
        previous &&
        previous.left === next.left &&
        previous.width === next.width &&
        previous.top === next.top &&
        previous.bottom === next.bottom &&
        previous.maxHeight === next.maxHeight
          ? previous
          : next,
      );
    };
    measure();
    window.addEventListener("resize", measure);
    window.addEventListener("scroll", measure, true);
    return () => {
      window.removeEventListener("resize", measure);
      window.removeEventListener("scroll", measure, true);
    };
  }, [anchor, active]);
  return box;
}

function optionDomId(id: string, index: number): string {
  return `${id}-option-${index}`;
}

/**
 * A WAI-ARIA 1.2 combobox with a list popup: a text input that filters nothing itself and a
 * listbox of async options. Keys: Down/Up move (and wrap), Enter picks the highlighted option,
 * Escape closes, Home/End jump to the first or last option while one is highlighted, Tab leaves.
 * Focus never leaves the input (`aria-activedescendant`). Loading and empty states are rows inside
 * a polite live region, so they are announced.
 */
function Combobox({
  id,
  inputValue,
  onInputValueChange,
  options,
  onSelect,
  loading = false,
  emptyText,
  loadingText,
  notice,
  autoActivateFirst = false,
  openOnFocus = true,
  inline = false,
  endAdornment,
  disabled,
  placeholder,
  autoFocus,
  name,
  className,
  inputClassName,
  inputRef,
  onInputKeyDown,
  onFocus,
  onBlur,
  onOpenChange,
  listLabel,
  "aria-label": ariaLabel,
  "aria-labelledby": ariaLabelledBy,
  "aria-describedby": ariaDescribedBy,
  "aria-invalid": ariaInvalid,
  "data-testid": testId,
}: ComboboxProps) {
  const [openState, setOpenState] = React.useState(false);
  const [activeIndex, setActiveIndex] = React.useState(-1);
  const anchorRef = React.useRef<HTMLDivElement>(null);
  const open = inline || openState;
  const listboxId = `${id}-listbox`;

  const setOpen = React.useCallback(
    (next: boolean) => {
      setOpenState(next);
      onOpenChange?.(next);
    },
    [onOpenChange],
  );

  // The highlight follows the list: a new list starts at the first option (or none).
  const optionKey = options.map((option) => option.value).join("\u0000");
  // biome-ignore lint/correctness/useExhaustiveDependencies: `optionKey` is the trigger
  React.useEffect(() => {
    setActiveIndex(autoActivateFirst && options.length > 0 ? 0 : -1);
  }, [optionKey, autoActivateFirst]);

  const box = usePopupBox(anchorRef, open && !inline);

  const activeOption = activeIndex >= 0 ? options[activeIndex] : undefined;
  React.useEffect(() => {
    if (!open || activeIndex < 0) {
      return;
    }
    document.getElementById(optionDomId(id, activeIndex))?.scrollIntoView?.({ block: "nearest" });
  }, [open, activeIndex, id]);

  const choose = (option: ComboboxOption) => {
    if (option.disabled) {
      return;
    }
    onSelect(option);
    if (!inline) {
      setOpen(false);
    }
  };

  const move = (delta: 1 | -1) => {
    if (options.length === 0) {
      return;
    }
    setActiveIndex((current) => {
      if (current < 0) {
        return delta === 1 ? 0 : options.length - 1;
      }
      return (current + delta + options.length) % options.length;
    });
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    onInputKeyDown?.(event);
    if (event.defaultPrevented) {
      return;
    }
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        if (!open) {
          setOpen(true);
          setActiveIndex(options.length > 0 ? 0 : -1);
        } else {
          move(1);
        }
        break;
      case "ArrowUp":
        event.preventDefault();
        if (!open) {
          setOpen(true);
          setActiveIndex(options.length > 0 ? options.length - 1 : -1);
        } else {
          move(-1);
        }
        break;
      case "Home":
        if (open && activeIndex >= 0) {
          event.preventDefault();
          setActiveIndex(0);
        }
        break;
      case "End":
        if (open && activeIndex >= 0) {
          event.preventDefault();
          setActiveIndex(options.length - 1);
        }
        break;
      case "Enter":
        if (open && activeOption) {
          event.preventDefault();
          choose(activeOption);
        }
        break;
      case "Escape":
        if (open && !inline) {
          event.preventDefault();
          event.stopPropagation();
          setOpen(false);
        }
        break;
      case "Tab":
        if (!inline) {
          setOpen(false);
        }
        break;
      default:
        break;
    }
  };

  const showEmpty = !loading && options.length === 0;
  const hasStatus = loading || (showEmpty && emptyText !== "") || options.length > 0;
  const grouped = options.some((option) => option.group !== undefined);
  const countText =
    options.length === 1
      ? formatMessage("combobox.resultOne")
      : formatMessage("combobox.resultMany", { count: options.length });

  const rows: React.ReactNode[] = [];
  let previousGroup: string | undefined;
  options.forEach((option, index) => {
    if (grouped && option.group !== previousGroup) {
      previousGroup = option.group;
      rows.push(
        <div
          key={`group-${option.group ?? ""}-${option.value}`}
          id={`${id}-group-${index}`}
          className="px-3 pt-2 pb-1 text-xs font-medium text-muted-foreground"
          role="presentation"
        >
          {option.group}
        </div>,
      );
    }
    const active = index === activeIndex;
    rows.push(
      // biome-ignore lint/a11y/useKeyWithClickEvents: keyboard access is the input's (aria-activedescendant)
      <div
        key={option.value}
        id={optionDomId(id, index)}
        role="option"
        tabIndex={-1}
        aria-selected={active}
        aria-disabled={option.disabled ? true : undefined}
        data-active={active ? "true" : undefined}
        onMouseDown={(event) => event.preventDefault()}
        onMouseMove={() => {
          if (!active) {
            setActiveIndex(index);
          }
        }}
        onClick={() => choose(option)}
        className={cn(
          "flex min-h-10 cursor-pointer items-center gap-3 px-3 py-1.5 text-left",
          active && "bg-accent text-accent-foreground",
          option.disabled && "cursor-not-allowed opacity-50",
        )}
      >
        {option.icon ? (
          <span className="shrink-0 text-muted-foreground" aria-hidden="true">
            {option.icon}
          </span>
        ) : null}
        <span className="grid min-w-0 flex-1 gap-0.5">
          <span className="text-sm leading-snug font-semibold [overflow-wrap:anywhere]">
            {option.label}
          </span>
          {option.description ? (
            <span className="text-[13px] leading-snug text-muted-foreground [overflow-wrap:anywhere]">
              {option.description}
            </span>
          ) : null}
        </span>
        {option.trailing ? <span className="shrink-0">{option.trailing}</span> : null}
      </div>,
    );
  });

  const list = (
    <div
      data-slot="combobox-popup"
      className={cn(
        "overflow-y-auto overscroll-contain text-popover-foreground",
        inline ? "max-h-[min(50vh,360px)]" : "rounded-md border border-border bg-popover",
      )}
      style={inline || !box ? undefined : { maxHeight: box.maxHeight }}
    >
      {options.length > 0 ? (
        <div
          id={listboxId}
          role="listbox"
          aria-label={listLabel ?? ariaLabel}
          aria-labelledby={listLabel ? undefined : ariaLabelledBy}
          className="py-1"
        >
          {rows}
        </div>
      ) : null}
      <div role="status" aria-live="polite" aria-atomic="true">
        {loading ? (
          <p className="flex items-center gap-2 px-3 py-2.5 text-muted-foreground">
            <Spinner />
            <span>{loadingText ?? formatMessage("combobox.loading")}</span>
          </p>
        ) : showEmpty ? (
          emptyText === "" ? null : (
            <p className="px-3 py-2.5 text-muted-foreground">
              {emptyText ?? formatMessage("combobox.empty")}
            </p>
          )
        ) : (
          <span className="sr-only">{countText}</span>
        )}
      </div>
      {notice}
    </div>
  );

  const popupVisible = open && (hasStatus || notice !== undefined);

  return (
    <div
      ref={anchorRef}
      data-slot="combobox"
      className={cn("relative min-w-0", inline && "grid gap-2", className)}
    >
      <div className="relative">
        <Input
          ref={inputRef as React.Ref<HTMLInputElement> | undefined}
          id={id}
          name={name}
          role="combobox"
          aria-autocomplete="list"
          aria-haspopup="listbox"
          aria-expanded={popupVisible}
          aria-controls={popupVisible && options.length > 0 ? listboxId : undefined}
          aria-activedescendant={
            popupVisible && activeOption ? optionDomId(id, activeIndex) : undefined
          }
          aria-label={ariaLabel}
          aria-labelledby={ariaLabelledBy}
          aria-describedby={ariaDescribedBy}
          aria-invalid={ariaInvalid}
          data-testid={testId}
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
          disabled={disabled}
          placeholder={placeholder}
          autoFocus={autoFocus}
          value={inputValue}
          className={cn(endAdornment ? "pr-9" : undefined, inputClassName)}
          onChange={(event) => {
            onInputValueChange(event.target.value);
            if (!inline) {
              setOpen(true);
            }
          }}
          onFocus={(event) => {
            if (openOnFocus && !inline) {
              setOpen(true);
            }
            onFocus?.(event);
          }}
          onClick={() => {
            if (!inline && !disabled) {
              setOpen(true);
            }
          }}
          onBlur={(event) => {
            if (!inline) {
              setOpen(false);
            }
            onBlur?.(event);
          }}
          onKeyDown={onKeyDown}
        />
        {endAdornment ? (
          <span className="absolute inset-y-0 right-1 flex items-center">{endAdornment}</span>
        ) : null}
      </div>
      {popupVisible
        ? inline
          ? list
          : createPortal(
              <div
                style={{
                  position: "fixed",
                  left: box?.left ?? 0,
                  top: box?.top,
                  bottom: box?.bottom,
                  width: box?.width ?? 0,
                  visibility: box ? "visible" : "hidden",
                }}
                className="z-50"
                data-slot="combobox-layer"
              >
                {list}
              </div>,
              document.body,
            )
        : null}
    </div>
  );
}

export { Combobox, type ComboboxOption, type ComboboxProps };
