import { notFound, redirect } from "next/navigation";

import { PageRails } from "@/components/saved-views/PageRails";
import { apiGet, ApiError } from "@/lib/api/fetch";
import type { PageView } from "@/lib/api/types";

/** Multi-page rails M5: per-page detail. Renders the same
 *  `<PageRails>` component as `/` but scoped to the page identified by
 *  `slug`. The system page (slug `home`) redirects to `/` so it stays
 *  the canonical Home URL — `/pages/home` would otherwise show the
 *  same content under a less-stable path.
 *
 *  Slug resolution runs server-side via `/me/pages`; we accept the
 *  small per-render cost because the route shell already needs the
 *  page record for the header + the not-found branch. */
export default async function PageDetailRoute({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;

  let pages: PageView[];
  try {
    pages = await apiGet<PageView[]>("/me/pages");
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) {
      redirect("/sign-in");
    }
    throw e;
  }
  const page = pages.find((p) => p.slug === slug);
  if (!page) notFound();
  if (page.is_system) redirect("/");

  return (
    <PageRails
      pageId={page.id}
      pageName={page.name}
      pageDescription={page.description ?? null}
      isSystem={page.is_system}
      showInSidebar={page.show_in_sidebar}
    />
  );
}
