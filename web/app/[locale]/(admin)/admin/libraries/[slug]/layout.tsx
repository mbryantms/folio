import { notFound } from "next/navigation";

import { LibraryTabs } from "@/components/admin/library/LibraryTabs";
import { apiGet, ApiError } from "@/lib/api/fetch";
import type { LibraryView } from "@/lib/api/types";

export default async function LibraryDetailLayout({
  children,
  params,
}: {
  children: React.ReactNode;
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
  const basePath = `/admin/libraries/${slug}`;
  return (
    <div className="space-y-6">
      <header className="space-y-1">
        <p className="text-muted-foreground text-xs font-medium tracking-widest uppercase">
          Library
        </p>
        <h1 className="text-foreground text-2xl font-semibold tracking-tight">
          {library.name}
        </h1>
        <p className="text-muted-foreground font-mono text-xs break-all">
          {library.root_path}
        </p>
      </header>
      <LibraryTabs basePath={basePath} />
      <div className="pt-4">{children}</div>
    </div>
  );
}
