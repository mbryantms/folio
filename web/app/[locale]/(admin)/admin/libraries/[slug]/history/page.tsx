import { ScanRunsTable } from "@/components/admin/library/ScanRunsTable";

export default async function LibraryHistoryPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;
  return <ScanRunsTable libraryId={slug} />;
}
