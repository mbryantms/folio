/**
 * Banner text for error codes the server's no-JavaScript form paths put on
 * the auth pages' URL (`?error=<code>`). Codes without an entry keep the
 * page's own wording.
 */
export const REQUEST_NOT_VERIFIED =
  "That request could not be verified. Reload the page and try again.";

export function authErrorText(code: string | null | undefined): string | null {
  return code === "auth.csrf" ? REQUEST_NOT_VERIFIED : null;
}
