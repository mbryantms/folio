export type Connectivity =
  "reachable" | "offline" | "unreachable" | "authentication";
let last: Connectivity | undefined;
export function reportConnectivity(status: Connectivity) {
  if (typeof window === "undefined" || status === last) return;
  last = status;
  window.dispatchEvent(
    new CustomEvent("folio:connectivity", { detail: status }),
  );
}
export async function connectionFetch(
  input: RequestInfo | URL,
  init?: RequestInit,
) {
  try {
    const response = await fetch(input, init);
    reportConnectivity(response.status >= 500 ? "unreachable" : "reachable");
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
