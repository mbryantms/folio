/**
 * Issue page › Library & file: a non-admin's issue response carries no
 * `file_path`, so the File row is built with a `null` value and the grid
 * leaves it out; an admin's response still shows the path.
 */
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";

import { MetadataGrid } from "@/components/library/MetadataGrid";

function fileItems(filePath: string | null | undefined) {
  return [
    { label: "File size", value: "12 MB" },
    {
      label: "File",
      value: filePath ? createElement("code", null, filePath) : null,
      wide: true,
    },
  ];
}

describe("issue File row", () => {
  it("is left out when the response has no file_path", () => {
    const html = renderToStaticMarkup(
      createElement(MetadataGrid, { items: fileItems(undefined) }),
    );
    expect(html).toContain("File size");
    expect(html).not.toContain(">File<");
    expect(html).not.toContain("<code");
  });

  it("shows the path to an admin", () => {
    const html = renderToStaticMarkup(
      createElement(MetadataGrid, {
        items: fileItems("/library/Saga/Saga 001.cbz"),
      }),
    );
    expect(html).toContain("/library/Saga/Saga 001.cbz");
  });
});
