import { redirect } from "next/navigation";

export default async function LibraryThumbnailsPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;
  redirect(`/admin/libraries/${slug}/scan`);
}
