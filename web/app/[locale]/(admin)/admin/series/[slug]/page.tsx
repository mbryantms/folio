import { SeriesIdentityForm } from "@/components/admin/series/SeriesIdentityForm";

export default async function AdminSeriesPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;
  return <SeriesIdentityForm id={slug} />;
}
