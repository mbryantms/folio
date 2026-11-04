"use client";

/**
 * "Theme switch reveal" (folio-flair-1.0, standard).
 *
 * Changing the theme spreads the new one across the screen in a circle
 * from the control that asked for it (or from the last pointer press),
 * using a root view transition: the old page stays as a still snapshot
 * while the new one is clipped to a growing circle. Reduced motion (OS or
 * the account's Motion setting) and browsers without view transitions get
 * the plain instant swap.
 *
 * next-themes applies its attribute from an effect, which would land after
 * the transition has already captured the "new" state, so the resolved
 * theme is written to `<html>` here first; next-themes then sets the same
 * value.
 */
type Theme = "system" | "dark" | "light" | "amber" | (string & {});

let lastPress: { x: number; y: number } | null = null;
let tracking = false;

function track() {
  if (tracking || typeof window === "undefined") return;
  tracking = true;
  window.addEventListener(
    "pointerdown",
    (e) => {
      lastPress = { x: e.clientX, y: e.clientY };
    },
    { capture: true, passive: true },
  );
}
track();

function reduced(): boolean {
  return (
    document.documentElement.getAttribute("data-motion") === "reduced" ||
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

function resolve(theme: Theme): string {
  if (theme !== "system") return theme;
  return window.matchMedia("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

/** Radius that reaches the farthest viewport corner from (x, y). */
export function revealRadius(
  x: number,
  y: number,
  w: number,
  h: number,
): number {
  return Math.hypot(Math.max(x, w - x), Math.max(y, h - y));
}

export function revealTheme(
  theme: Theme,
  setTheme: (t: string) => void,
  origin?: Element | null,
): void {
  const root = document.documentElement;
  const apply = () => {
    const resolved = resolve(theme);
    root.setAttribute("data-theme", resolved);
    root.style.colorScheme = resolved === "light" ? "light" : "dark";
    setTheme(theme);
  };
  const from = root.getAttribute("data-theme");
  if (
    typeof document.startViewTransition !== "function" ||
    reduced() ||
    from === resolve(theme)
  ) {
    setTheme(theme);
    return;
  }

  const r = origin?.getBoundingClientRect();
  const x = r ? r.left + r.width / 2 : (lastPress?.x ?? window.innerWidth - 48);
  const y = r ? r.top + r.height / 2 : (lastPress?.y ?? 32);
  const end = revealRadius(x, y, window.innerWidth, window.innerHeight);

  root.classList.add("theme-reveal");
  const vt = document.startViewTransition(apply);
  vt.ready
    .then(() => {
      root.animate(
        {
          clipPath: [
            `circle(0px at ${x}px ${y}px)`,
            `circle(${end}px at ${x}px ${y}px)`,
          ],
        },
        {
          duration: 560,
          easing: "cubic-bezier(0.2, 0.8, 0.2, 1)",
          pseudoElement: "::view-transition-new(root)",
        },
      );
    })
    .catch(() => {});
  vt.finished
    .catch(() => {})
    .finally(() => root.classList.remove("theme-reveal"));
}
