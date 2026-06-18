import { StatusScreen } from "@/components/StatusScreen";
import { Button } from "@/components/ui/button";

/**
 * Root-level not-found, for `notFound()` thrown outside the `[locale]`
 * subtree (or an unmatched root path). Renders inside the root layout,
 * so it inherits the theme. Uses a hard `<a>` since it sits above the
 * locale router segment.
 */
export default function RootNotFound() {
  return (
    <StatusScreen
      code="404"
      title="Page not found"
      description="That page doesn't exist, or you don't have access to it. If you were looking for a specific series or issue, try searching for it."
      actions={
        <>
          <Button asChild>
            <a href="/">Back to library</a>
          </Button>
          <Button asChild variant="outline">
            <a href="/search">Search the library</a>
          </Button>
        </>
      }
    />
  );
}
