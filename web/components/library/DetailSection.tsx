/**
 * A titled card inside a Details tab (issue + series pages). Each category
 * (Publication / Format / Genres / Library / External IDs) is its own card —
 * the same `bg-card` chrome the issue Metadata tab uses — so the tab reads as
 * a set of grouped panels rather than one long flat list.
 */
export function DetailSection({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="border-border bg-card space-y-3 rounded-lg border p-4">
      <h3 className="text-foreground text-sm font-semibold">{title}</h3>
      {children}
    </section>
  );
}
