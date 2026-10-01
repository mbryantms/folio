import { EntityIndexPage } from "@/components/library/EntityPageServer";

/** `/publishers` browse index (WP-5.5). */
export default function PublishersIndexPage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | undefined>>;
}) {
  return <EntityIndexPage kind="publishers" searchParams={searchParams} />;
}
