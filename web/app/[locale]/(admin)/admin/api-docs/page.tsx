import { PageHeader } from "@/components/admin/PageHeader";
import { ApiDocsViewer } from "@/components/admin/ApiDocsViewer";

export default function ApiDocsPage() {
  return (
    <div className="space-y-6">
      <PageHeader
        title="API reference"
        description="Live OpenAPI 3.0 spec served by the Rust backend at /openapi.json. Browse endpoints, schemas, and try requests inline."
      />
      <ApiDocsViewer />
    </div>
  );
}
