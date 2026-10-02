import { PageHeader } from "@/components/admin/PageHeader";
import { RelationshipSuggestionsPanel } from "@/components/admin/relationships/RelationshipSuggestionsPanel";

export default async function RelationshipsPage() {
  return (
    <>
      <PageHeader
        title="Relationships"
        description="Suggested links between series — sequels, spin-offs, crossovers, collected editions — found from evidence already in your libraries. Accept, change the kind, or reject; rejected suggestions are never proposed again unless you reopen them."
      />
      <RelationshipSuggestionsPanel />
    </>
  );
}
