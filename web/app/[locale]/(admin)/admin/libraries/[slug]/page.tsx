import { LibraryOverview } from "@/components/admin/library/LibraryOverview";

export default async function LibraryOverviewPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;
  return <LibraryOverview id={slug} />;
}
