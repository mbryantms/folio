export type Connectivity =
  "reachable" | "offline" | "unreachable" | "authentication";
/** Window event carrying a `Connectivity` as `detail`, fired on change. */
export const CONNECTIVITY_EVENT = "folio:connectivity";
let last: Connectivity | undefined;
export function reportConnectivity(status: Connectivity) {
  if (typeof window === "undefined" || status === last) return;
  last = status;
  window.dispatchEvent(new CustomEvent(CONNECTIVITY_EVENT, { detail: status }));
}
export async function connectionFetch(
  input: RequestInfo | URL,
  init?: RequestInit,
) {
  try {
    const response = await fetch(input, init);
    reportConnectivity(
      response.status >= 500 && !isBusyOcrAnswer(input, response)
        ? "unreachable"
        : "reachable",
    );
    return response;
  } catch (error) {
    if (!(error instanceof DOMException && error.name === "AbortError")) {
      reportConnectivity(
        typeof navigator !== "undefined" && !navigator.onLine
          ? "offline"
          : "unreachable",
      );
    }
    throw error;
  }
}

/** A 503 + `Retry-After` from the OCR routes means the OCR workers are
 *  busy, not that the server is out of reach: the page-text panel retries
 *  those, and counting them would flicker the connectivity indicator.
 *  Every other 5xx still counts. */
function isBusyOcrAnswer(input: RequestInfo | URL, response: Response) {
  if (response.status !== 503 || !response.headers.get("retry-after")) {
    return false;
  }
  const url =
    typeof input === "string"
      ? input
      : input instanceof URL
        ? input.href
        : input.url;
  const path = url.split("?")[0] ?? "";
  return /\/me\/issues\/[^/]+\/(ocr|pages\/\d+\/text-regions)$/.test(path);
}
