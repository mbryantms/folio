"use client";

/**
 * "Covers fly into lists" (folio-flair-1.0, standard). After a successful
 * add to Want to Read or a collection, a copy of the cover you acted on
 * arcs into the matching sidebar item, which then pulses. On phones (no
 * sidebar) it lands on the bottom bar's More tab; with neither visible it
 * does nothing. Reduced motion skips it.
 *
 * The add is triggered from a card's menu, long-press sheet or a dialog,
 * so the origin is the last card the user pressed: a capture-phase
 * pointerdown/keydown listener remembers it.
 */
let lastCard: HTMLElement | null = null;
let listening = false;

function remember(e: Event) {
  const t = e.target as Element | null;
  const card = t?.closest?.<HTMLElement>("[data-card]");
  if (card) lastCard = card;
}

/** Start remembering the last pressed card. Idempotent. */
export function trackCardOrigin(): void {
  if (listening || typeof document === "undefined") return;
  listening = true;
  document.addEventListener("pointerdown", remember, true);
  document.addEventListener("keydown", remember, true);
}

export type FlyTarget = "want_to_read" | "collections";

function isVisible(el: Element | null): el is HTMLElement {
  if (!el) return false;
  const r = el.getBoundingClientRect();
  return (
    r.width > 0 && r.height > 0 && r.bottom > 0 && r.top < window.innerHeight
  );
}

function reduced(): boolean {
  return (
    document.documentElement.dataset.motion === "reduced" ||
    !!window.matchMedia?.("(prefers-reduced-motion: reduce)").matches
  );
}

export function flyCoverToNav(target: FlyTarget): void {
  if (typeof document === "undefined" || reduced()) return;
  const img = lastCard?.querySelector<HTMLImageElement>("img");
  if (!img || !isVisible(img)) return;
  const dest = [
    document.querySelector(`[data-tour="nav-${target}"]`),
    document.querySelector(`[data-tour="tab-more"]`),
  ].find(isVisible);
  if (!dest) return;

  const from = img.getBoundingClientRect();
  const to = dest.getBoundingClientRect();
  const size = Math.min(28, to.height);
  const ghost = img.cloneNode() as HTMLImageElement;
  Object.assign(ghost.style, {
    position: "fixed",
    left: `${from.left}px`,
    top: `${from.top}px`,
    width: `${from.width}px`,
    height: `${from.height}px`,
    objectFit: "cover",
    borderRadius: "6px",
    zIndex: "60",
    pointerEvents: "none",
    boxShadow: "0 10px 30px rgb(0 0 0 / 0.35)",
    transformOrigin: "top left",
  });
  ghost.removeAttribute("srcset");
  ghost.alt = "";
  document.body.appendChild(ghost);

  const dx = to.left + 12 - from.left;
  const dy = to.top + (to.height - size) / 2 - from.top;
  const s = size / from.height;
  // A shallow arc: rise first, then drop into the item.
  const lift = Math.min(120, Math.abs(dy) * 0.35 + 40);
  const anim = ghost.animate(
    [
      { transform: "translate(0, 0) scale(1) rotate(0deg)", opacity: 1 },
      {
        transform: `translate(${dx * 0.45}px, ${dy * 0.45 - lift}px) scale(${(1 + s) / 2}) rotate(-6deg)`,
        opacity: 1,
        offset: 0.45,
      },
      {
        transform: `translate(${dx}px, ${dy}px) scale(${s}) rotate(0deg)`,
        opacity: 0.2,
      },
    ],
    { duration: 650, easing: "cubic-bezier(0.45, 0, 0.25, 1)" },
  );
  const done = () => {
    ghost.remove();
    dest.animate(
      [
        { transform: "scale(1)" },
        { transform: "scale(1.06)" },
        { transform: "scale(1)" },
      ],
      { duration: 320, easing: "ease-out" },
    );
  };
  anim.finished.then(done, done);
}
