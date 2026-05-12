import { LiveScanProgress } from "@/components/admin/library/LiveScanProgress";

export default async function LibraryLiveScanPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;
  return <LiveScanProgress libraryId={slug} />;
}
