// Types for the helpers `tests/pwa/serwist-config.test.ts` imports.
export interface ManifestEntry {
  url: string;
  revision: string | null;
  size?: number;
}
export function buildId(path?: string): string | null;
export function stampEntries(
  entries: ManifestEntry[],
  id: string | null,
): ManifestEntry[];
declare const config: Record<string, unknown>;
export default config;
