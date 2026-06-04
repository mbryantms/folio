"use client";

import { ApiReferenceReact } from "@scalar/api-reference-react";
import "@scalar/api-reference-react/style.css";

/**
 * Embedded Scalar API reference for the admin section. Points at
 * the live `/openapi.json` served by the Rust backend
 * (`crates/server/src/api/meta.rs`), so the docs always reflect
 * the running server — no codegen step required.
 *
 * The Scalar bundle is client-only (Vue under the hood); the
 * `"use client"` directive plus its own CSS keep it isolated from
 * the rest of the app. We render it inside an admin-page container
 * so the layout/sidebar still wrap it.
 */
export function ApiDocsViewer() {
  return (
    <div className="border-border bg-card rounded-md border">
      <ApiReferenceReact
        configuration={{
          url: "/openapi.json",
          // Hide the dark/light toggle since the rest of the admin
          // section is dark-locked. Scalar still respects our
          // surrounding card chrome.
          hideDarkModeToggle: true,
          // Hide the "Powered by" footer — we're embedding inside
          // a self-hosted admin surface, not a marketing page.
          hideClientButton: true,
        }}
      />
    </div>
  );
}
