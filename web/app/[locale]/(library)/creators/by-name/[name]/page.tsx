import { redirect } from "next/navigation";

import { apiGet, ApiError } from "@/lib/api/fetch";

/** Name → slug resolver. Credit pills emit
 *  `/creators/by-name/<URL-encoded-name>` instead of computing the
 *  slug client-side (which would diverge from the backend's
 *  collision-suffix rules and break for the long-tail `-2`/`-3`
 *  slugs).
 *
 *  Lookup hits `/api/creators/resolve?name=…`, which returns the
 *  canonical slug or 404. On hit → permanent redirect to
 *  `/creators/<slug>` (so the URL bar settles on the canonical
 *  form). On miss → degrade gracefully to the library-grid
 *  any-role credits filter, which still gives the user something
 *  resembling the right answer.
 */
export default async function CreatorByNamePage({
  params,
}: {
  params: Promise<{ name: string }>;
}) {
  const { name } = await params;
  const decoded = decodeURIComponent(name);
  try {
    const r = await apiGet<{ slug: string }>(
      `/creators/resolve?name=${encodeURIComponent(decoded)}`,
    );
    redirect(`/creators/${encodeURIComponent(r.slug)}`);
  } catch (e) {
    if (e instanceof ApiError) {
      if (e.status === 401) redirect(`/sign-in`);
      if (e.status === 404) {
        // No backfilled person row for this credit — fall through to
        // the legacy library-grid any-role view. Still shows the
        // user the creator's work; just no detail page yet.
        redirect(
          `/?library=all&credits=${encodeURIComponent(decoded)}`,
        );
      }
    }
    throw e;
  }
}
