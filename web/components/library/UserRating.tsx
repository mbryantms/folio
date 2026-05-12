"use client";

import * as React from "react";
import { useRouter } from "next/navigation";

import { RatingStars } from "@/components/ui/rating-stars";
import { useSetIssueRating, useSetSeriesRating } from "@/lib/api/mutations";
import { cn } from "@/lib/utils";

type Common = {
  /** Initial rating from the server (0..=5 or null). The component owns
   *  optimistic state from there so a click feels instant. */
  initial: number | null;
  /** Label rendered next to the stars. Defaults to "Your rating". */
  label?: string;
  /** Layout mode:
   *  - `default` — horizontal row (label · stars · value), spacious.
   *  - `compact` — vertical stack with small stars, fits in a stats card.
   *  - `inline`  — outline-pill that visually matches `<Badge variant="outline">`,
   *    so the widget can sit alongside status badges. */
  variant?: "default" | "compact" | "inline";
  className?: string;
};

type IssueProps = Common & {
  scope: "issue";
  seriesSlug: string;
  issueSlug: string;
};

type SeriesProps = Common & {
  scope: "series";
  seriesSlug: string;
};

/**
 * Self-contained rating widget for the issue and series pages. Owns the
 * optimistic state — a star click updates locally first, then PUT-fires
 * the mutation. On success we refresh the route so server-derived state
 * (audit log, future "average rating", etc.) stays current.
 */
export function UserRating(props: IssueProps | SeriesProps) {
  const router = useRouter();
  const [rating, setRating] = React.useState<number | null>(props.initial);

  // Re-sync from props when the server-rendered page reloads with a new
  // value (e.g. after `router.refresh()`). Keeps the widget in step with
  // anything that changes the rating outside this component.
  React.useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setRating(props.initial);
  }, [props.initial]);

  const seriesMut = useSetSeriesRating(
    props.scope === "series" ? props.seriesSlug : "__inactive__",
  );
  const issueMut = useSetIssueRating(
    props.scope === "issue" ? props.seriesSlug : "__inactive__",
    props.scope === "issue" ? props.issueSlug : "__inactive__",
  );
  const mut = props.scope === "series" ? seriesMut : issueMut;

  const onChange = (next: number | null) => {
    setRating(next);
    mut.mutate(
      { rating: next },
      {
        onSuccess: () => {
          router.refresh();
        },
        onError: () => {
          // Server rejected (validation or transient) — roll back so the
          // displayed value never lies about persistence.
          setRating(props.initial);
        },
      },
    );
  };

  const label = props.label ?? "Your rating";
  const valueLabel =
    rating == null
      ? "Not rated"
      : rating === Math.floor(rating)
        ? `${rating}.0 / 5`
        : `${rating} / 5`;

  const variant = props.variant ?? "default";

  if (variant === "inline") {
    // Pill that visually matches `<Badge variant="outline">` so it can
    // sit alongside status badges in the issue / series header. Stars
    // are sized to align with the badge's text-xs label, and the
    // numeric value collapses to a slash-suffixed glyph (3.5/5) when set
    // so the row stays compact.
    return (
      <div
        className={cn(
          "border-border text-foreground inline-flex items-center gap-1.5 rounded-md border px-2 py-0.5",
          props.className,
        )}
      >
        <RatingStars
          value={rating}
          onChange={onChange}
          size="sm"
          label={label}
        />
        <span className="text-muted-foreground text-xs leading-none font-medium">
          {rating == null
            ? "Rate"
            : rating === Math.floor(rating)
              ? `${rating}.0`
              : `${rating}`}
        </span>
      </div>
    );
  }

  if (variant === "compact") {
    return (
      <div className={cn("flex flex-col gap-1", props.className)}>
        <span className="text-muted-foreground text-xs font-medium tracking-wider uppercase">
          {label}
        </span>
        <RatingStars
          value={rating}
          onChange={onChange}
          size="sm"
          label={label}
        />
        <span className="text-muted-foreground text-xs">{valueLabel}</span>
      </div>
    );
  }

  return (
    <div className={cn("flex items-center gap-3", props.className)}>
      <span className="text-muted-foreground text-xs font-medium tracking-wider uppercase">
        {label}
      </span>
      <RatingStars value={rating} onChange={onChange} size="md" label={label} />
      <span className="text-muted-foreground text-xs">{valueLabel}</span>
    </div>
  );
}
