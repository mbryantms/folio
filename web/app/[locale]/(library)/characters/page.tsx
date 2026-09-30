import { EntityIndexPage } from "@/components/library/EntityPageServer";

/** `/characters` browse index (WP-5.5). */
export default function CharactersIndexPage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | undefined>>;
}) {
  return <EntityIndexPage kind="characters" searchParams={searchParams} />;
}
