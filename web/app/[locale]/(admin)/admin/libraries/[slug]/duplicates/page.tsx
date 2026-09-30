import { DuplicatesPanel } from "@/components/admin/library/DuplicatesPanel";

export default async function LibraryDuplicatesPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;
  return <DuplicatesPanel libraryId={slug} />;
}
