/**
 * Single-cover component used by the library grid, series page, issue page.
 *
 * Falls back to a placeholder tile (publisher / state) when `src` is null —
 * this keeps the layout stable for issues with no cover (encrypted, malformed,
 * or thumbnail not yet generated).
 */
export function Cover({
  src,
  alt,
  fallback,
  className,
}: {
  src: string | null | undefined;
  alt: string;
  fallback?: string | null;
  className?: string;
}) {
  const cls =
    "aspect-[2/3] bg-neutral-900 rounded-md border border-neutral-800 overflow-hidden";
  if (src) {
    return (
      // eslint-disable-next-line @next/next/no-img-element
      <img
        src={src}
        alt={alt}
        loading="lazy"
        decoding="async"
        className={`${cls} w-full object-cover ${className ?? ""}`}
      />
    );
  }
  return (
    <div
      role="img"
      aria-label={alt}
      className={`${cls} grid place-items-center text-neutral-600 ${className ?? ""}`}
    >
      <span className="px-2 text-center text-xs tracking-widest uppercase">
        {fallback ?? "—"}
      </span>
    </div>
  );
}
