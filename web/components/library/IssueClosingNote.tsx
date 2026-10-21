"use client";

import * as React from "react";

import { ClosingNoteEditor } from "@/components/library/ClosingNoteEditor";
import { useSetIssueNote } from "@/lib/api/mutations";

/**
 * Issue-page host for the caller's issue-level closing note. Owns the
 * stored value (optimistic) and the PUT; `ClosingNoteEditor` owns the
 * draft. Sits under the rating / badge row in the issue header.
 */
export function IssueClosingNote({
  seriesSlug,
  issueSlug,
  initial,
  className,
}: {
  seriesSlug: string;
  issueSlug: string;
  initial: string | null;
  className?: string;
}) {
  const [value, setValue] = React.useState<string | null>(initial);
  React.useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setValue(initial);
  }, [initial]);
  const mut = useSetIssueNote(seriesSlug, issueSlug);
  return (
    <ClosingNoteEditor
      variant="page"
      id="issue-closing-note"
      value={value}
      saving={mut.isPending}
      className={className}
      onSave={(body) => {
        const prev = value;
        setValue(body);
        mut.mutate(
          { body },
          {
            onSuccess: (view) => setValue(view?.body ?? null),
            onError: () => setValue(prev),
          },
        );
      }}
    />
  );
}
