import { notFound, redirect } from "next/navigation";

import { EntityDetail } from "@/components/library/EntityDetail";
import { EntityIndex } from "@/components/library/EntityIndex";
import { parseStartsWithParam } from "@/components/library/library-grid-filters";
import { apiGet, ApiError } from "@/lib/api/fetch";
import type { EntityKindPath } from "@/lib/api/queries";
import type { EntityDetailView } from "@/lib/api/types";
import { signInUrl } from "@/lib/api/sign-in-url";

/** Server body shared by `/characters/[slug]`, `/teams/[slug]`,
 *  `/arcs/[slug]` and `/publishers/[slug]` (WP-5.5). Fetches the header
 *  payload server-side (404 → `notFound()`, including entities the
 *  caller can't see; 401 → sign-in) and hands off to the client grids. */
export async function EntityDetailPage({
  kind,
  params,
}: {
  kind: EntityKindPath;
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;
  let detail: EntityDetailView;
  try {
    detail = await apiGet<EntityDetailView>(
      `/${kind}/${encodeURIComponent(slug)}`,
    );
  } catch (e) {
    if (e instanceof ApiError) {
      if (e.status === 401) redirect(await signInUrl());
      if (e.status === 404) notFound();
    }
    throw e;
  }
  return <EntityDetail kind={kind} detail={detail} />;
}

/** Server body shared by the four browse-index pages. */
export async function EntityIndexPage({
  kind,
  searchParams,
}: {
  kind: EntityKindPath;
  searchParams: Promise<Record<string, string | undefined>>;
}) {
  const sp = await searchParams;
  return (
    <EntityIndex
      kind={kind}
      initialStartsWith={parseStartsWithParam(sp.starts_with) ?? null}
    />
  );
}
