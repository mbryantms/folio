/**
 * Human copy for a non-`active` issue parse state (`issue.state` as the
 * scanner writes it: `encrypted` / `malformed`; soft removal is
 * `removed_at`, not a state). Shared by the issue card badge and the issue
 * page so a password-protected file reads as what it is instead of the raw
 * state token (audit 2026-11 N-L1). Unknown states fall back to the token.
 */

type StateCopy = {
  /** Short badge text. */
  label: string;
  /** One-line "why can't I read this". */
  hint: string;
};

const STATE_COPY: Record<string, StateCopy> = {
  encrypted: {
    label: "Password-protected",
    hint: "Folio can't read encrypted archives. Repack the file without a password and rescan.",
  },
  malformed: {
    label: "Unreadable archive",
    hint: "The archive or its ComicInfo.xml could not be parsed. See library health for the file.",
  },
};

/** Badge text for an issue state (falls back to the raw state). */
export function issueStateLabel(state: string): string {
  return STATE_COPY[state]?.label ?? state;
}

/** Explanation for an unreadable issue state, if we have one. */
export function issueStateHint(state: string): string | undefined {
  return STATE_COPY[state]?.hint;
}
