/** View-transition names shared by a card cover, the detail-page loading
 *  slot and the detail page's header cover (see `lib/ui/cover-morph.ts`).
 *  Plain module, so server components can call them. */
export const seriesCoverName = (seriesSlug: string) =>
  `series-cover-${seriesSlug}`;
export const issueCoverName = (seriesSlug: string, issueSlug: string) =>
  `issue-cover-${seriesSlug}-${issueSlug}`;
