import { EntityDetailPage } from "@/components/library/EntityPageServer";

/** `/locations/<slug>` landing page. */
export default function LocationPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  return <EntityDetailPage kind="locations" params={params} />;
}
