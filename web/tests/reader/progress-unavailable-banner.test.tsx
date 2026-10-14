/**
 * Post-audit T2.2: a failed saved-position fetch is surfaced, not
 * silently treated as "start at page 0".
 */
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";

import { ProgressUnavailableBanner } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/ProgressUnavailableBanner";

describe("<ProgressUnavailableBanner>", () => {
  it("names the failure and offers Retry", () => {
    const html = renderToStaticMarkup(
      createElement(ProgressUnavailableBanner, { onRetry: () => undefined }),
    );
    expect(html).toMatch(/role="alert"/);
    expect(html).toMatch(/Couldn(&#x27;|')t load your saved position/);
    expect(html).toMatch(/Retry/);
    expect(html).toMatch(/aria-label="Dismiss"/);
  });
});
