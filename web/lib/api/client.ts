/**
 * Typed API client. Generated types live in `./types.ts` (run `just openapi`).
 *
 * The client always sends cookies (httpOnly session) and the X-CSRF-Token header
 * on unsafe verbs (§17.3). The token is read from the non-httpOnly __Host-comic_csrf
 * cookie that the server sets on /auth/me.
 */
import createClient from "openapi-fetch";

// `paths` will be regenerated into `./types.ts` after the first `just openapi` run.
// Until then, an empty paths object keeps openapi-fetch happy at the type level.
type Paths = Record<string, never>;

function getCsrfToken(): string | null {
  if (typeof document === "undefined") return null;
  const m = document.cookie.match(/(?:^|;\s*)__Host-comic_csrf=([^;]+)/);
  return m ? decodeURIComponent(m[1]!) : null;
}

const baseClient = createClient<Paths>({
  // Browser: bare same-origin paths (Rust binary is the public origin
  // as of v0.2). SSR: explicit Rust hostname so the request reaches
  // the backend regardless of where Next is running.
  baseUrl: typeof window === "undefined" ? "http://localhost:8080" : "",
  credentials: "include",
});

// Wrap fetch to inject CSRF on unsafe verbs.
baseClient.use({
  onRequest({ request }) {
    if (!["GET", "HEAD", "OPTIONS"].includes(request.method)) {
      const csrf = getCsrfToken();
      if (csrf) request.headers.set("X-CSRF-Token", csrf);
    }
    return request;
  },
});

export const api = baseClient;
