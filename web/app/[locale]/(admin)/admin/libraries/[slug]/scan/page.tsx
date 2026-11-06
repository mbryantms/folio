import { LiveScanProgress } from "@/components/admin/library/LiveScanProgress";
import { ScanRunsTable } from "@/components/admin/library/ScanRunsTable";

/** A library's Scan tab: the live scan on top, its run history below.
 *  Where a single-library scan lands. */
export default async function LibraryScanPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;
  return (
    <div className="space-y-8">
      <LiveScanProgress libraryId={slug} />
      <section className="space-y-3" aria-labelledby="scan-runs-heading">
        <h2 id="scan-runs-heading" className="text-lg font-semibold">
          Run history
        </h2>
        <ScanRunsTable libraryId={slug} />
      </section>
    </div>
  );
}
