/**
 * Shared discriminated-union scope for the metadata-match surfaces.
 *
 * Lives in its own module (rather than `MetadataMatchDialog.tsx`) so the
 * extracted hooks — `useMetadataCandidateSearch`, `useMetadataApplyWait` —
 * can take it without an import cycle back through the dialog component.
 * `MetadataMatchDialog` re-exports it, so existing
 * `import { MetadataMatchScope } from ".../MetadataMatchDialog"` sites keep
 * working.
 */
export type MetadataMatchScope =
  | { kind: "series"; seriesSlug: string; libraryId: string }
  | { kind: "issue"; seriesSlug: string; issueSlug: string; libraryId: string };
