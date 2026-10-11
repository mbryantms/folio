import { EntityIndexPage } from "@/components/library/EntityPageServer";

/** `/locations` browse index. */
export default function LocationsIndexPage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | undefined>>;
}) {
  return <EntityIndexPage kind="locations" searchParams={searchParams} />;
}
