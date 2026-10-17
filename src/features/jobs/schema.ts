import { z } from "zod";

/**
 * UX pre-validation only. Rust validates and normalizes every value
 * it stores and refuses what these schemas let through. The messages are catalogue ids.
 */

/** The posting form: the text must not be blank once trimmed (no request is sent). */
export const pasteFormSchema = z.object({
  text: z.string().refine((value) => value.trim() !== "", { message: "job.validation.paste" }),
});

/** The search form: the query must not be blank once trimmed; the UI cap is 512 characters. */
export const searchFormSchema = z.object({
  query: z
    .string()
    .max(512, { message: "job.validation.search" })
    .refine((value) => value.trim() !== "", { message: "job.validation.search" }),
});

export type PasteFormValues = z.infer<typeof pasteFormSchema>;
export type SearchFormValues = z.infer<typeof searchFormSchema>;
