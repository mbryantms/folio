import { EntityIndexPage } from "@/components/library/EntityPageServer";

/** `/arcs` browse index (WP-5.5). */
export default function ArcsIndexPage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | undefined>>;
}) {
  return <EntityIndexPage kind="arcs" searchParams={searchParams} />;
}
