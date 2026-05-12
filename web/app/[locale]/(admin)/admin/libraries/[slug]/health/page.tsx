import { HealthIssuesTable } from "@/components/admin/library/HealthIssuesTable";

export default async function LibraryHealthPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;
  return <HealthIssuesTable libraryId={slug} />;
}
