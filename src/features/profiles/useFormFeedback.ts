import { useCallback, useEffect, useMemo, useState } from "react";
import { formatMessage } from "../../i18n/messages";
import type { FieldError, FieldErrors } from "./validation";

/** What a field needs to show its error: the text once it is due, and the blur to report. */
export type FormFeedback = {
  /** The error text of an input, or undefined while it is not due (untouched, or valid). */
  errorFor: (id: string) => string | undefined;
  /** Marks an input as visited, so its error shows from now on. */
  touch: (id: string) => void;
};

export type FormFeedbackController = {
  feedback: FormFeedback;
  /**
   * Called when the operator presses Save. Makes every error due and moves focus to the first
   * invalid input; returns true when there is one, so the caller does not save.
   */
  blocked: boolean;
  attempt: () => boolean;
};

/** The error as the field shows it. */
export function errorText(error: FieldError): string {
  return formatMessage(error.id, error.params);
}

/**
 * Shows each error only when it is due: after the operator left that input, or after a Save was
 * pressed. A new baseline (`resetKey`: another profile loaded, or a save finished) starts clean, so
 * a fresh form does not open covered in errors.
 */
export function useFormFeedback(errors: FieldErrors, resetKey: unknown): FormFeedbackController {
  const [touched, setTouched] = useState<ReadonlySet<string>>(new Set());
  const [attempted, setAttempted] = useState(false);

  // biome-ignore lint/correctness/useExhaustiveDependencies: resetKey is the trigger, not a value read
  useEffect(() => {
    setTouched(new Set());
    setAttempted(false);
  }, [resetKey]);

  const touch = useCallback((id: string) => {
    setTouched((current) => (current.has(id) ? current : new Set(current).add(id)));
  }, []);

  const feedback = useMemo<FormFeedback>(
    () => ({
      errorFor: (id) => {
        const error = errors.get(id);
        return error !== undefined && (attempted || touched.has(id)) ? errorText(error) : undefined;
      },
      touch,
    }),
    [errors, attempted, touched, touch],
  );

  const attempt = useCallback(() => {
    setAttempted(true);
    const [first] = errors.keys();
    if (first === undefined) {
      return false;
    }
    document.getElementById(first)?.focus();
    return true;
  }, [errors]);

  return { feedback, blocked: attempted && errors.size > 0, attempt };
}
