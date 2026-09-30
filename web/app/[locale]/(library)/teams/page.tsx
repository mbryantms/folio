import { EntityIndexPage } from "@/components/library/EntityPageServer";

/** `/teams` browse index (WP-5.5). */
export default function TeamsIndexPage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | undefined>>;
}) {
  return <EntityIndexPage kind="teams" searchParams={searchParams} />;
}
