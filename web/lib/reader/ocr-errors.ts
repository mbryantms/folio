/**
 * Why reading a page's text failed, in plain words — the one place those
 * words live. Used by text capture (marker overlay + editor) and the
 * page-text panel for both server requests: text detection
 * (`GET …/pages/{n}/text-regions`) and recognition (`POST …/ocr`).
 *
 * A failure is never "no text": the server answering 200 with empty text
 * is a result ("no text here"); anything else is a failure with a reason.
 * Transient failures (busy, rate-limited, timed out, offline library, a
 * dropped connection) are never cached as a result and can be retried;
 * the rest say what has to change first.
 *
 * Error envelope: `{ "error": { "code", "message" } }` (`docs/dev/ocr.md`).
 * The 408 from the server's request timeout has no body, so the status
 * alone decides there.
 */
import { MAX_RETRY_AFTER_MS, retryAfterMs } from "@/lib/api/retry-after";

/** Status used for a request that never got an HTTP answer. */
export const NETWORK_FAILURE = 0;

/** A text request that failed (not one that found no text). */
export class OcrRequestError extends Error {
  constructor(
    public readonly status: number,
    public readonly code: string | null,
    /** The server's `Retry-After` in ms, when it sent one. */
    public readonly retryAfterMs: number | null = null,
  ) {
    super(ocrFailureMessage(status, code));
    this.name = "OcrRequestError";
  }

  get transient(): boolean {
    return isTransientOcrFailure(this.status, this.code);
  }
}

/** Build an {@link OcrRequestError} from a failed response. */
export async function ocrErrorFromResponse(
  res: Response,
): Promise<OcrRequestError> {
  let code: string | null = null;
  try {
    const body = (await res.json()) as { error?: { code?: unknown } };
    if (typeof body?.error?.code === "string") code = body.error.code;
  } catch {
    // No JSON body (the 408 timeout, a proxy page): the status decides.
  }
  const retryAfter = res.headers.get("retry-after");
  return new OcrRequestError(
    res.status,
    code,
    retryAfter ? retryAfterMs(retryAfter) : null,
  );
}

/** Normalise anything a text request can throw (an
 *  {@link OcrRequestError}, a query's `HttpError` with `status` / `code` /
 *  `retryAfter`, a network `TypeError`) to an {@link OcrRequestError}. */
export function toOcrRequestError(err: unknown): OcrRequestError {
  if (err instanceof OcrRequestError) return err;
  if (err && typeof err === "object" && "status" in err) {
    const e = err as {
      status: unknown;
      code?: unknown;
      retryAfter?: unknown;
    };
    if (typeof e.status === "number") {
      return new OcrRequestError(
        e.status,
        typeof e.code === "string" ? e.code : null,
        typeof e.retryAfter === "string" ? retryAfterMs(e.retryAfter) : null,
      );
    }
  }
  return new OcrRequestError(NETWORK_FAILURE, null);
}

/** Whether asking again later can succeed without anything changing. */
export function isTransientOcrFailure(
  status: number,
  code: string | null,
): boolean {
  if (status === NETWORK_FAILURE) return true;
  if (status === 408 || status === 429 || status === 503) return true;
  return code === "service_unavailable" || code === "library_offline";
}

/** The reason a text request failed, in words a reader can act on. */
export function ocrFailureMessage(status: number, code: string | null): string {
  switch (code) {
    case "archive_needs_conversion":
      return "This issue's file has to be converted to CBZ before its text can be read.";
    case "archive_missing":
      return "This issue's file is missing, so its text can't be read.";
    case "archive_unreadable":
      return "This issue's file can't be opened (it's damaged or encrypted), so its text can't be read.";
    case "library_offline":
      return "The library's folder is offline right now. Try again once it's back.";
    case "service_unavailable":
      return "The server is busy reading other text. Try again in a moment.";
    case "rate_limited":
      return "Too many text requests in a short time. Try again in a moment.";
    case "ocr_model_unverified":
      return "The text recognition model couldn't be downloaded or failed its check. An admin can find the cause in the server log.";
    case "ocr_failed":
      return "Text recognition failed on the server.";
    case "decode_failed":
      return "This page isn't an image the server can read.";
    case "not_found":
      return "This page couldn't be found.";
    case "invalid_region":
      return "That area isn't on the page.";
    default:
      break;
  }
  switch (status) {
    case NETWORK_FAILURE:
      return "Couldn't reach the server. Check your connection and try again.";
    case 408:
      return "Reading the text took too long. Try again.";
    case 409:
      return "This issue's file has to be converted to CBZ before its text can be read.";
    case 410:
      return "This issue's file is missing or damaged, so its text can't be read.";
    case 429:
      return "Too many text requests in a short time. Try again in a moment.";
    case 503:
      return "The server is busy. Try again in a moment.";
    default:
      return `Couldn't read the text (error ${status}).`;
  }
}

/** Automatic retries the page-text panel makes for a transient failure. */
export const OCR_AUTO_RETRIES = 3;

/** TanStack `retryDelay`: TanStack passes the number of failures BEFORE
 *  the one being retried (0 for the first retry). Waits the server's
 *  `Retry-After` when it sent one, else 2 s, 4 s, 8 s; never over 30 s. */
export function ocrRetryDelayMs(failureCount: number, err: unknown): number {
  const e = toOcrRequestError(err);
  const backoff = 2_000 * 2 ** Math.max(0, failureCount);
  return Math.min(MAX_RETRY_AFTER_MS, e.retryAfterMs ?? backoff);
}

/** TanStack `retry` predicate: transient failures, up to `max` times. */
export function retryTransientOcr(max: number) {
  return (failureCount: number, err: unknown) =>
    failureCount < max && toOcrRequestError(err).transient;
}
