/**
 * Header facts as a muted text line ("Ongoing · Teen · TPB"), for the
 * text header style. Empty facts are skipped; nothing renders when none
 * are left.
 */
export function HeroFactsLine({
  facts,
}: {
  facts: (string | null | undefined | false)[];
}) {
  const shown = facts.filter((f): f is string => !!f);
  if (shown.length === 0) return null;
  return (
    <p className="text-muted-foreground text-sm">
      {shown.map((f, i) => (
        <span key={`${i}-${f}`}>
          {i > 0 && (
            <span aria-hidden className="px-1.5 opacity-60">
              ·
            </span>
          )}
          {f}
        </span>
      ))}
    </p>
  );
}
