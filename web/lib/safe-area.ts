/**
 * Top safe-area correction for installed (standalone) web apps.
 *
 * The app lays itself out edge to edge (`viewport-fit=cover`) and pads
 * every top bar by `--safe-top`, which `globals.css` seeds from
 * `env(safe-area-inset-top)`. That is right whenever the web view really
 * runs under the status bar. Since iOS / iPadOS 26.1 it usually does not:
 * the OS reserves an opaque status bar of its own for home-screen web
 * apps (the `black-translucent` style lost its effect) while WebKit keeps
 * reporting the old inset. Trusting `env()` then pads the top twice — a
 * blank band under the system bar on every page, and in the reader a
 * dark scrim across the chrome.
 *
 * `env()` cannot be second-guessed from CSS, but geometry can: in
 * standalone mode the viewport spans the full screen when it runs under
 * the status bar, and is shorter by the bar's height when the OS has
 * reserved it. `reservedTopInset` measures that and, when the OS is
 * already holding the space, the probe pins `--safe-top` to 0 so every
 * consumer (topbar height, reader chrome padding, status-bar scrim)
 * collapses together. Off standalone, or when the viewport genuinely
 * runs edge to edge (iPhone, iPadOS ≤ 26.0), `env()` stays in charge.
 *
 * Stage Manager / iPadOS windowed mode is out of scope: there the window
 * is smaller than the screen for its own reasons and WebKit reports no
 * inset for the window controls at all (known WebKit gap).
 */

/** Anything at least this tall between screen edge and viewport top is
 *  the OS reserving a status bar (iPad: ~24pt, iPhone: ≥ 44pt). */
export const MIN_RESERVED_STATUS_BAR_PX = 20;

export interface ViewportGeometry {
  /** `window.innerWidth` */
  innerWidth: number;
  /** `window.innerHeight` */
  innerHeight: number;
  /** `screen.width` / `screen.height` — on iOS these do not swap with
   *  orientation, so both are passed and matched by aspect. */
  screenWidth: number;
  screenHeight: number;
}

/**
 * Pixels the OS has taken off the top of the viewport for its own status
 * bar, inferred from how much shorter the viewport is than the screen in
 * the current orientation. Returns 0 when the viewport is full height
 * (page runs under the status bar) or the geometry is unusable.
 */
export function reservedTopInset(g: ViewportGeometry): number {
  if (
    ![g.innerWidth, g.innerHeight, g.screenWidth, g.screenHeight].every(
      (n) => n > 0,
    )
  ) {
    return 0;
  }
  const landscape = g.innerWidth > g.innerHeight;
  const long = Math.max(g.screenWidth, g.screenHeight);
  const short = Math.min(g.screenWidth, g.screenHeight);
  const fullHeight = landscape ? short : long;
  return Math.max(0, fullHeight - g.innerHeight);
}

/**
 * The `--safe-top` value to pin on `:root`, or `null` to leave the
 * stylesheet's `env()`-driven default alone.
 */
export function safeTopOverride(
  g: ViewportGeometry,
  standalone: boolean,
): string | null {
  if (!standalone) return null;
  return reservedTopInset(g) >= MIN_RESERVED_STATUS_BAR_PX ? "0px" : null;
}
