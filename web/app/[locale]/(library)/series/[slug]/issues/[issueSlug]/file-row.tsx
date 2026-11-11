import type { ReactNode } from "react";

/**
 * The "File" row of the issue page's Library & file grid. The archive
 * path is server detail that only an admin's response carries; without
 * it the row's value is `null` and the grid leaves the row out.
 */
export function fileRowItem(filePath: string | null | undefined): {
  label: string;
  value: ReactNode;
  wide: boolean;
} {
  return {
    label: "File",
    value: filePath ? <FilePathValue path={filePath} /> : null,
    wide: true,
  };
}

function FilePathValue({ path }: { path: string }) {
  return (
    <code className="bg-muted/50 block rounded-md px-3 py-2 font-mono text-xs break-all">
      {path}
    </code>
  );
}
