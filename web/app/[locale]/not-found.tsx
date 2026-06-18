import Link from "next/link";

import { StatusScreen } from "@/components/StatusScreen";
import { Button } from "@/components/ui/button";

export default function LocaleNotFound() {
  return (
    <StatusScreen
      code="404"
      title="Page not found"
      description="That page doesn't exist, or you don't have access to it. If you were looking for a specific series or issue, try searching for it."
      actions={
        <>
          <Button asChild>
            <Link href="/">Back to library</Link>
          </Button>
          <Button asChild variant="outline">
            <Link href="/search">Search the library</Link>
          </Button>
        </>
      }
    />
  );
}
