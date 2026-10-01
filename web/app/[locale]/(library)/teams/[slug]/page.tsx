import { EntityDetailPage } from "@/components/library/EntityPageServer";

/** `/teams/<slug>` landing page (WP-5.5). */
export default function TeamPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  return <EntityDetailPage kind="teams" params={params} />;
}
