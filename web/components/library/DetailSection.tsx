/**
 * A titled block inside a Details tab (issue + series pages). Keeps the
 * section heading style and spacing consistent across the
 * Publication / Format / Genres / Library groups so the tab reads as a set
 * of categories rather than one long flat list.
 */
export function DetailSection({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="space-y-3">
      <h3 className="text-foreground text-sm font-semibold">{title}</h3>
      {children}
    </section>
  );
}
