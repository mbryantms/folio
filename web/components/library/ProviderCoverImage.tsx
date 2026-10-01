"use client";

/**
 * `<ProviderCoverImage>` — a provider-CDN cover (`<img>`) that falls back
 * to the grey placeholder when the image can't load.
 *
 * Provider cover URLs are hotlinks to a third-party CDN. Some refuse
 * them outright — GCD's `files1.comics.org` answers every hotlink with a
 * Cloudflare bot challenge (HTTP 403) — and any CDN can 404 a stale
 * URL. Without a fallback the browser paints a broken-image glyph plus
 * the alt text; this swaps in the same placeholder a candidate with no
 * cover URL gets. Applies to every provider, not just GCD.
 *
 * The failed URL is remembered (not a boolean), so a new `src` gets a
 * fresh attempt without an effect to reset state.
 */

import { useState } from "react";

export function ProviderCoverImage({
  src,
  alt,
  className,
  placeholderClassName,
}: {
  src: string | null | undefined;
  alt: string;
  /** Classes for the `<img>`. */
  className?: string;
  /** Classes for the placeholder `<div>` (defaults to `className`). */
  placeholderClassName?: string;
}) {
  const [failedSrc, setFailedSrc] = useState<string | null>(null);
  if (!src || failedSrc === src) {
    return (
      <div
        className={placeholderClassName ?? className}
        aria-hidden
        data-testid="provider-cover-placeholder"
      />
    );
  }
  return (
    // eslint-disable-next-line @next/next/no-img-element
    <img
      src={src}
      alt={alt}
      loading="lazy"
      // A hotlink-refusing CDN must not leak our page URL either.
      referrerPolicy="no-referrer"
      onError={() => setFailedSrc(src)}
      className={className}
    />
  );
}
