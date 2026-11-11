/**
 * Issue page › Library & file: a non-admin's issue response carries no
 * `file_path`, so the page's File row has a `null` value and the grid
 * leaves it out; an admin's response still shows the path.
 */
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";

import { MetadataGrid } from "@/components/library/MetadataGrid";
import { fileRowItem } from "@/app/[locale]/(library)/series/[slug]/issues/[issueSlug]/file-row";

function renderRows(filePath: string | null | undefined) {
  return renderToStaticMarkup(
    createElement(MetadataGrid, {
      items: [{ label: "File size", value: "12 MB" }, fileRowItem(filePath)],
    }),
  );
}

describe("issue File row", () => {
  it("is left out when the response has no file_path", () => {
    expect(fileRowItem(undefined).value).toBeNull();
    const html = renderRows(undefined);
    expect(html).toContain("File size");
    expect(html).not.toContain(">File<");
    expect(html).not.toContain("<code");
  });

  it("shows the path to an admin", () => {
    const html = renderRows("/library/Saga/Saga 001.cbz");
    expect(html).toContain(">File<");
    expect(html).toContain("/library/Saga/Saga 001.cbz");
  });
});
