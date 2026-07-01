/**
 * Shared mutation primitives — extracted into `_core.ts` so per-domain
 * shards (`thumbnails.ts`, etc.) can import them without circular
 * `./index` references. M5 of code-quality-cleanup-1.0.
 *
 * Anything cross-cutting (CSRF probing, the typed `ApiMutationError`,
 * the toast / retry-action wiring inside `useApiMutation`) lives here.
 * Per-domain hooks compose on top via `useApiMutation(build, options)`.
 */
import * as React from "react";
import { useMutation, type UseMutationOptions } from "@tanstack/react-query";
import { toast } from "sonner";

import { apiFetch } from "../auth-refresh";

export function getCsrfToken(): string | null {
  if (typeof document === "undefined") return null;
  const m = document.cookie.match(/(?:^|;\s*)(?:__Host-)?comic_csrf=([^;]+)/);
  return m ? decodeURIComponent(m[1]!) : null;
}

export type ApiMutationInput = {
  path: string;
  method: "POST" | "PATCH" | "PUT" | "DELETE";
  body?: unknown;
};

/**
 * One field-scoped validation failure from a 422 response's
 * `error.details` array. Mirrors the Rust `shared::error::FieldError`
 * wire shape. `field` is the dotted path (e.g. `"port"`, `"smtp.host"`);
 * an empty string means a whole-body/form-level error.
 *
 * Frontend-only type (the OpenAPI surface doesn't schematise the error
 * envelope), so it lives here rather than in the generated types.
 */
export type ApiFieldError = { field: string; message: string };

/**
 * Structured error thrown by `apiMutate`. Carries the HTTP status (or
 * `"network"` when the request never reached the server) so callers
 * can branch on transience. `useApiMutation`'s `onError` reads
 * `.transient` to decide whether to attach a Retry action to the
 * error toast. On a 422 it also carries `fields` — the per-field
 * validation messages, which `applyServerErrors` binds onto a form.
 */
export class ApiMutationError extends Error {
  readonly status: number | "network";
  /** Field-level validation failures from `error.details` (422 only;
   *  empty otherwise). */
  readonly fields: ApiFieldError[];
  /** Machine-readable `error.code` from the envelope (e.g.
   *  `"metadata.no_providers"`), so call sites can branch on the
   *  specific failure rather than matching on the human message.
   *  `null` for network errors / non-enveloped responses. */
  readonly code: string | null;
  /** Server request id (`x-request-id` response header), surfaced in the
   *  error toast so a user can quote it and an operator can grep it in
   *  /admin/logs (OBS-1). `null` for network errors / missing header. */
  readonly requestId: string | null;

  constructor(
    message: string,
    status: number | "network",
    fields: ApiFieldError[] = [],
    code: string | null = null,
    requestId: string | null = null,
  ) {
    super(message);
    this.name = "ApiMutationError";
    this.status = status;
    this.fields = fields;
    this.code = code;
    this.requestId = requestId;
  }

  /**
   * `true` for failures the user can plausibly retry by clicking
   * a button: network errors (offline / DNS / TLS hiccup) and 5xx
   * server errors (transient backend issue, restart in progress).
   * `false` for 4xx — those are validation / auth / permission
   * problems where retrying without changing input won't help.
   */
  get transient(): boolean {
    return (
      this.status === "network" ||
      (typeof this.status === "number" && this.status >= 500)
    );
  }
}

export async function apiMutate<T>({
  path,
  method,
  body,
}: ApiMutationInput): Promise<T | null> {
  const csrf = getCsrfToken();
  // Only declare `Content-Type: application/json` when there's an
  // actual JSON body to send. axum's `Option<Json<T>>` extractor
  // accepts a missing body cleanly but errors with 400 ("EOF while
  // parsing a value") when the header is present and the body is
  // empty — biting the legacy "Pin to home" pill on /settings/views
  // which posts no body to /me/saved-views/{id}/pin.
  const hasBody = body !== undefined;
  let res: Response;
  try {
    res = await apiFetch(path, {
      method,
      headers: {
        Accept: "application/json",
        ...(hasBody ? { "Content-Type": "application/json" } : {}),
        ...(csrf ? { "X-CSRF-Token": csrf } : {}),
      },
      body: hasBody ? JSON.stringify(body) : undefined,
    });
  } catch (e) {
    // `fetch` rejects on network errors (offline, DNS, TLS, CORS
    // preflight refusal). Promote to a typed retryable error so the
    // toast can offer a Retry action.
    const msg = e instanceof Error ? e.message : "Network error";
    throw new ApiMutationError(msg, "network");
  }
  if (!res.ok) {
    let detail = "";
    let fields: ApiFieldError[] = [];
    let code: string | null = null;
    try {
      const errBody = (await res.json()).error;
      detail = errBody?.message ?? `${res.status}`;
      code = typeof errBody?.code === "string" ? errBody.code : null;
      if (Array.isArray(errBody?.details)) {
        fields = errBody.details.filter(
          (d: unknown): d is ApiFieldError =>
            typeof d === "object" &&
            d !== null &&
            typeof (d as ApiFieldError).field === "string" &&
            typeof (d as ApiFieldError).message === "string",
        );
      }
    } catch {
      detail = `${res.status}`;
    }
    throw new ApiMutationError(
      detail,
      res.status,
      fields,
      code,
      res.headers.get("x-request-id"),
    );
  }
  if (res.status === 204) return null;
  const text = await res.text();
  return text ? (JSON.parse(text) as T) : null;
}

export function useApiMutation<TData, TInput>(
  build: (input: TInput) => ApiMutationInput,
  options?: Omit<
    UseMutationOptions<TData | null, Error, TInput>,
    "mutationFn"
  > & {
    successMessage?: string | ((data: TData | null, input: TInput) => string);
    /**
     * Stable sonner toast `id`. When set, rapid-fire calls to the
     * same mutation (e.g. bulk progress flipping read/unread back
     * and forth) collapse into a single toast that updates in place
     * rather than stacking N toasts. Use for bulk operations and
     * autosave-like surfaces where each click is one logical event,
     * not N. Sonner reuses the same toast element when an id repeats.
     */
    toastId?: string;
  },
) {
  const { successMessage, toastId, onSuccess, onError, ...rest } =
    options ?? {};
  // Ref to the mutation's `mutate` so the error-toast Retry action
  // can re-fire the same request without forcing every call site to
  // wire up its own onError handler. The ref is populated after the
  // `useMutation` call below; by the time onError fires (network
  // round-trip later), the ref is set.
  const mutateRef = React.useRef<((input: TInput) => void) | null>(null);
  const mutation = useMutation<TData | null, Error, TInput>({
    mutationFn: (input) => apiMutate<TData>(build(input)),
    // react-query v5.79+ appends a 4th `context` arg to mutation
    // lifecycle callbacks: (data, variables, onMutateResult, context).
    // The 3rd arg (onMutate's return) keeps its position; forward all
    // four so caller-supplied callbacks get the full signature.
    onSuccess: (data, input, onMutateResult, context) => {
      if (successMessage) {
        const msg =
          typeof successMessage === "function"
            ? successMessage(data, input)
            : successMessage;
        toast.success(msg, toastId ? { id: toastId } : undefined);
      }
      onSuccess?.(data, input, onMutateResult, context);
    },
    onError: (err, input, onMutateResult, context) => {
      // Attach Retry only for transient failures (5xx + network).
      // 4xx errors are user-facing validation/auth/permission issues
      // that retrying without changing input won't fix.
      const transient = err instanceof ApiMutationError && err.transient;
      const requestId =
        err instanceof ApiMutationError ? err.requestId : null;
      toast.error(err.message, {
        ...(toastId ? { id: toastId } : {}),
        ...(requestId ? { description: `Reference: ${requestId}` } : {}),
        ...(transient && {
          action: {
            label: "Retry",
            onClick: () => mutateRef.current?.(input),
          },
        }),
      });
      onError?.(err, input, onMutateResult, context);
    },
    ...rest,
  });
  // Populate the ref out-of-render so React 19's strict-mode lint
  // (no `.current` writes during render) stays happy. `mutation.mutate`
  // is stable across renders, so the effect fires once per hook
  // instance; by the time onError can fire (network round-trip
  // later), the ref is set.
  React.useEffect(() => {
    mutateRef.current = mutation.mutate;
  }, [mutation.mutate]);
  return mutation;
}
