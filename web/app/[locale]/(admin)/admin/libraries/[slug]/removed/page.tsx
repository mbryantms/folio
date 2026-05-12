import { RemovedItemsTable } from "@/components/admin/library/RemovedItemsTable";

export default async function LibraryRemovedPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;
  return <RemovedItemsTable libraryId={slug} />;
}
