"use client";

import { useRouter, useSearchParams } from "next/navigation";
import { useId, useState, useTransition, type FormEvent } from "react";
import { Search } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

/**
 * Server-driven search box. Submits the form via router push so the RSC page
 * re-renders with the new `?q=` query string. Uses the same shadcn Input/Button
 * primitives as the library grid toolbar so it sits flush in an inline toolbar.
 *
 * Two layouts:
 *   - default: input + Search button + Clear button. Used in page-level
 *     toolbars on the home page and search results page.
 *   - `compact`: input with embedded search icon, no separate buttons.
 *     Enter submits; the native `type="search"` clear (×) handles reset.
 *     Used in the mobile topbar so search is always one tap away.
 */
export function LibrarySearch({
  initial,
  basePath,
  compact = false,
  placeholder = "Search series…",
  className,
}: {
  initial: string;
  basePath: string;
  compact?: boolean;
  placeholder?: string;
  className?: string;
}) {
  const [value, setValue] = useState(initial);
  const [pending, start] = useTransition();
  const router = useRouter();
  const searchParams = useSearchParams();
  const inputId = useId();

  function navigate(nextValue: string) {
    const next = new URLSearchParams(searchParams);
    const trimmed = nextValue.trim();
    if (trimmed) {
      next.set("q", trimmed);
    } else {
      next.delete("q");
    }
    const qs = next.toString();
    start(() => {
      router.push(qs ? `${basePath}?${qs}` : basePath);
    });
  }

  function submit(e: FormEvent) {
    e.preventDefault();
    navigate(value);
  }

  if (compact) {
    return (
      <form
        onSubmit={submit}
        role="search"
        className={cn("relative flex-1", className)}
      >
        <label htmlFor={inputId} className="sr-only">
          Search library
        </label>
        <Search
          aria-hidden="true"
          className="text-muted-foreground pointer-events-none absolute top-1/2 left-2.5 h-4 w-4 -translate-y-1/2"
        />
        <Input
          id={inputId}
          type="search"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder={placeholder}
          disabled={pending}
          className="h-9 w-full pl-8"
          enterKeyHint="search"
        />
      </form>
    );
  }

  return (
    <form
      onSubmit={submit}
      role="search"
      className={cn("flex items-center gap-2", className)}
    >
      <label htmlFor={inputId} className="sr-only">
        Search library
      </label>
      <Input
        id={inputId}
        type="search"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        placeholder={placeholder}
        className="w-72"
      />
      <Button type="submit" variant="outline" size="sm" disabled={pending}>
        Search
      </Button>
      {initial ? (
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="text-muted-foreground"
          onClick={() => {
            setValue("");
            navigate("");
          }}
        >
          Clear
        </Button>
      ) : null}
    </form>
  );
}
