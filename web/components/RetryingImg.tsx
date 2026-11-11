"use client";

import { useEffect, useRef, useState, type ComponentProps } from "react";

/**
 * Back-off before each re-request of a failed thumbnail or cover. An
 * `<img>` error carries no status, so a 429 from the server's thumbnail
 * bucket (`docs/architecture/rate-limits.md`, refills within seconds)
 * looks the same as a missing file: two delayed tries cover the first,
 * and cost a truly missing image only two extra requests.
 */
export const IMAGE_RETRY_DELAYS_MS = [1_000, 3_000] as const;

/**
 * Retry state for one image source: `attempt` keys the `<img>` (a fresh
 * element re-requests the same URL, so the URL and every cache key stay
 * unchanged), and `retry()` schedules the next attempt, returning `false`
 * once the delays are spent. A new `src` starts over.
 */
export function useImageRetry(src: unknown): {
  attempt: number;
  retry: () => boolean;
} {
  const [state, setState] = useState({ src, attempt: 0 });
  const attempt = state.src === src ? state.attempt : 0;
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );
  const retry = () => {
    const delay = IMAGE_RETRY_DELAYS_MS[attempt];
    if (delay === undefined) return false;
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      timer.current = null;
      setState({ src, attempt: attempt + 1 });
    }, delay);
    return true;
  };
  return { attempt, retry };
}

/**
 * `<img>` that re-requests a failed load after a short back-off
 * ([`IMAGE_RETRY_DELAYS_MS`]) before giving up. `onError` fires only once
 * the retries are spent, so callers keep their "failed" handling as is.
 */
export function RetryingImg({ alt, onError, ...props }: ComponentProps<"img">) {
  const { attempt, retry } = useImageRetry(props.src);
  return (
    // eslint-disable-next-line @next/next/no-img-element
    <img
      key={attempt}
      alt={alt}
      {...props}
      onError={(event) => {
        if (!retry()) onError?.(event);
      }}
    />
  );
}
