import { EntityDetailPage } from "@/components/library/EntityPageServer";

/** `/arcs/<slug>` landing page (WP-5.5). */
export default function ArcPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  return <EntityDetailPage kind="arcs" params={params} />;
}
