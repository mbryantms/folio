/**
 * Human labels for the metadata field keys emitted by the Rust
 * `MetadataField::key()` impl + the sourceless `external_id` completeness key.
 * Shared by the Collection tab (missing-field lists) and the issue Metadata
 * tab (provenance table + pinned-field list) so labels never drift.
 */

const METADATA_FIELD_LABELS: Record<string, string> = {
  external_id: "Provider match",
  title: "Title",
  sort_name: "Sort name",
  series_type: "Series type",
  year_began: "Year began",
  year_end: "Year ended",
  volume: "Volume",
  deck: "Deck",
  description: "Description",
  summary: "Summary",
  notes: "Notes",
  scan_information: "Scan info",
  cover_date: "Cover date",
  store_date: "Store date",
  foc_date: "FOC date",
  page_count: "Page count",
  age_rating: "Age rating",
  format: "Format",
  language_code: "Language",
  manga: "Manga",
  price: "Price",
  sku: "SKU",
  community_rating: "Community rating",
  staff_rating: "Staff rating",
  aliases: "Aliases",
  status: "Status",
  publisher: "Publisher",
  imprint: "Imprint",
  credits: "Credits",
  characters: "Characters",
  teams: "Teams",
  locations: "Locations",
  concepts: "Concepts",
  objects: "Objects",
  story_arcs: "Story arcs",
  universes: "Universes",
  genres: "Genres",
  tags: "Tags",
  reprints: "Reprints",
  "cover.primary": "Cover",
  "cover.variants": "Cover variants",
};

/** Label a single metadata field key. Handles the `external_id.<source>`
 *  family and falls back to prettified snake_case for unknown keys. */
export function metadataFieldLabel(key: string): string {
  const known = METADATA_FIELD_LABELS[key];
  if (known) return known;
  if (key.startsWith("external_id.")) {
    return `${key.slice("external_id.".length)} ID`;
  }
  return key.replace(/_/g, " ");
}

/** Comma-join labels for a list of field keys. */
export function metadataFieldLabels(keys: string[]): string {
  return keys.map(metadataFieldLabel).join(", ");
}
