import { Construction } from "lucide-react";

import { EmptyState } from "./EmptyState";
import { PageHeader } from "./PageHeader";

export function PlaceholderPage({
  title,
  description,
  ships,
}: {
  title: string;
  description: string;
  ships: string;
}) {
  return (
    <>
      <PageHeader title={title} description={description} />
      <EmptyState
        icon={Construction}
        title={`Lands in ${ships}`}
        description="Foundation shipped in M1; this surface is wired up later in the plan."
      />
    </>
  );
}
