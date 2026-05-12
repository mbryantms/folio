import { notFound } from "next/navigation";

import { LibraryDangerZone } from "@/components/admin/library/LibraryDangerZone";
import { LibrarySettingsForm } from "@/components/admin/library/LibrarySettingsForm";
import { ApiError, apiGet } from "@/lib/api/fetch";
import type { LibraryView } from "@/lib/api/types";

export default async function LibrarySettingsPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;
  let library: LibraryView;
  try {
    library = await apiGet<LibraryView>(`/libraries/${slug}`);
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) notFound();
    throw e;
  }
  return (
    <div className="space-y-6">
      <LibrarySettingsForm id={slug} />
      <LibraryDangerZone id={slug} name={library.name} />
    </div>
  );
}
