/**
 * Bind a server 422's field-level validation errors onto a
 * react-hook-form instance, so each message lands inline under its
 * input instead of only in a toast (frontend-audit D3 / H2).
 *
 * The server's error envelope carries `error.details: [{field, message}]`
 * (see `shared::error::FieldError`); `apiMutate` parses it onto
 * `ApiMutationError.fields`. This helper maps those onto the form:
 * a named field gets `setError(field, …)`; an empty `field` (whole-body
 * rule) falls back to the form root so the message still surfaces.
 *
 * Adopted by the long admin forms in chunk 2.8. Returns `true` when at
 * least one field error was applied, so a caller can decide whether to
 * also toast a generic summary (don't double-report a field error that's
 * now inline).
 */
import type { FieldValues, Path, UseFormSetError } from "react-hook-form";

import { ApiMutationError } from "./mutations/_core";

export function applyServerErrors<TForm extends FieldValues>(
  setError: UseFormSetError<TForm>,
  err: unknown,
  /** Field names the form actually owns. When provided, server fields
   *  outside this set are routed to the form root instead of being
   *  silently dropped (a server field with no matching input would
   *  otherwise never display). */
  knownFields?: ReadonlyArray<Path<TForm>>,
): boolean {
  if (!(err instanceof ApiMutationError) || err.fields.length === 0) {
    return false;
  }
  const known = knownFields ? new Set<string>(knownFields) : null;
  let applied = false;
  for (const { field, message } of err.fields) {
    if (field && (!known || known.has(field))) {
      setError(field as Path<TForm>, { type: "server", message });
    } else {
      // Empty path (whole-body rule) or a field the form doesn't own:
      // surface on the form root so the message isn't lost.
      setError("root.serverError" as Path<TForm>, {
        type: "server",
        message: field ? `${field}: ${message}` : message,
      });
    }
    applied = true;
  }
  return applied;
}
