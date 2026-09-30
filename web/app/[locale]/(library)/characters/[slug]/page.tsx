import { EntityDetailPage } from "@/components/library/EntityPageServer";

/** `/characters/<slug>` landing page (WP-5.5). */
export default function CharacterPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  return <EntityDetailPage kind="characters" params={params} />;
}
