import { z } from "zod";

/**
 * UX pre-validation only. Rust validates and normalizes
 * every value on save and rejects what these schemas let through. The messages are catalogue ids.
 */

/** The search form: the query must not be blank once trimmed. */
export const searchFormSchema = z.object({
  query: z.string().refine((value) => value.trim() !== "", { message: "profileSearch.empty" }),
});

export type SearchFormValues = z.infer<typeof searchFormSchema>;
