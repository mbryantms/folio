"use client";

import { useRouter, useSearchParams } from "next/navigation";
import { useState, useTransition, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

/**
 * Server-driven search box. Submits the form via router push so the RSC page
 * re-renders with the new `?q=` query string. Uses the same shadcn Input/Button
 * primitives as the library grid toolbar so it sits flush in an inline toolbar.
 */
export function LibrarySearch({
  initial,
  basePath,
}: {
  initial: string;
  basePath: string;
}) {
  const [value, setValue] = useState(initial);
  const [pending, start] = useTransition();
  const router = useRouter();
  const searchParams = useSearchParams();

  function submit(e: FormEvent) {
    e.preventDefault();
    const next = new URLSearchParams(searchParams);
    if (value.trim()) {
      next.set("q", value.trim());
    } else {
      next.delete("q");
    }
    const qs = next.toString();
    start(() => {
      router.push(qs ? `${basePath}?${qs}` : basePath);
    });
  }

  return (
    <form onSubmit={submit} role="search" className="flex items-center gap-2">
      <label htmlFor="lib-q" className="sr-only">
        Search library
      </label>
      <Input
        id="lib-q"
        type="search"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        placeholder="Search series…"
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
            const next = new URLSearchParams(searchParams);
            next.delete("q");
            const qs = next.toString();
            start(() => {
              router.push(qs ? `${basePath}?${qs}` : basePath);
            });
          }}
        >
          Clear
        </Button>
      ) : null}
    </form>
  );
}
