import { EntityDetailPage } from "@/components/library/EntityPageServer";

/** `/publishers/<slug>` landing page (WP-5.5). */
export default function PublisherPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  return <EntityDetailPage kind="publishers" params={params} />;
}
