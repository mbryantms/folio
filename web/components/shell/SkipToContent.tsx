/**
 * Keyboard-only "Skip to main content" affordance. Hidden by default
 * (`sr-only`); first Tab on a fresh page surfaces it. Activating jumps
 * focus to the shell's `<main id="main-content">`, bypassing the long
 * sidebar nav. Renders nothing extra for sighted mouse users.
 */
export function SkipToContent() {
  return (
    <a
      href="#main-content"
      className="bg-background border-border text-foreground focus-visible:ring-ring sr-only rounded border px-3 py-2 text-sm shadow focus-visible:not-sr-only focus-visible:fixed focus-visible:top-2 focus-visible:left-2 focus-visible:z-50 focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:outline-none"
    >
      Skip to main content
    </a>
  );
}
