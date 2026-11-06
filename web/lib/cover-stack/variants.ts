/**
 * Cover-stack geometry: the five styles a user can pick for the fanned
 * covers on the Collections index and Settings → Views cards
 * (`users.cover_stack_style`, Settings → Theme).
 *
 * Each style is a `spec(depth, amount, fanned)` function (tuned in the
 * cover-stack lab, 2026-11-02) giving every cover a transform in
 * cover-width units: `tx`/`ty` translate (`ty` in cover heights), `rot`
 * degrees, `s` scale, and the pivot `ox`/`oy` as fractions of the cover
 * box. `layoutFor` turns that into percentage CSS:
 *
 * - The pivot is baked into the transform (`transform-origin: 0 0`,
 *   `translate(O+T) rotate scale translate(-O)`), so rest and fanned
 *   transforms share one function list and interpolate smoothly even
 *   when a style moves its pivot between states (Desk pile).
 * - The cover is sized so the union of every cover in both states fits
 *   the stack box, so fanning never overflows the card.
 * - Every style shares one box aspect (the tallest), so switching style
 *   never shifts the card layout.
 * - The resting stack is centred horizontally in the box; fanning spreads
 *   it across the box's full width.
 *
 * Percentages keep the stack fluid: it scales with whatever width the
 * card gives it, no measuring.
 */

export const COVER_STACK_STYLES = [
  "straight",
  "hand",
  "arc",
  "pile",
  "pickup",
] as const;
export type CoverStackStyle = (typeof COVER_STACK_STYLES)[number];
export const DEFAULT_COVER_STACK_STYLE: CoverStackStyle = "pickup";

export function isCoverStackStyle(v: unknown): v is CoverStackStyle {
  return (
    typeof v === "string" &&
    (COVER_STACK_STYLES as readonly string[]).includes(v)
  );
}

/** Covers behind the front one. */
export const BACK_COVERS = 3;
/** Cover aspect, height / width. */
const H = 1.5;

type Spec = {
  tx: number;
  ty: number;
  rot: number;
  s: number;
  ox: number;
  oy: number;
};

type Variant = {
  label: string;
  description: string;
  rest: number;
  fan: number;
  spec: (d: number, amount: number, fanned: boolean) => Spec;
};

const JITTER = [0, -2.6, 1.9, -1.4];

const spec = (p: Partial<Spec>): Spec => ({
  tx: 0,
  ty: 0,
  rot: 0,
  s: 1,
  ox: 0,
  oy: 0,
  ...p,
});

/** Tuned values from the lab export (rest / fanned amount per style). */
export const VARIANTS: Record<CoverStackStyle, Variant> = {
  straight: {
    label: "Straight",
    description: "Back covers step out to the right.",
    rest: 7,
    fan: 18,
    spec: (d, a) => spec({ tx: (d * a) / 100, s: 1 - 0.04 * d, oy: 0.5 }),
  },
  hand: {
    label: "Hand of cards",
    description: "Covers pivot from the bottom corner.",
    rest: 3.5,
    fan: 9,
    spec: (d, a) => spec({ rot: d * a, s: 1 - 0.03 * d, ox: 0.12, oy: 0.96 }),
  },
  arc: {
    label: "Arc",
    description: "Covers swing along a shallow arc.",
    rest: 1.25,
    fan: 2.75,
    spec: (d, a) => spec({ rot: d * a, s: 1 - 0.03 * d, ox: 0.5, oy: 2.4 }),
  },
  pile: {
    label: "Desk pile",
    description: "Askew at rest, squares up and spreads.",
    rest: 2,
    fan: 16,
    spec: (d, a, fanned) =>
      fanned
        ? spec({ tx: (d * a) / 100, rot: d * 1.2, s: 1 - 0.04 * d, oy: 1 })
        : spec({
            tx: d * 0.025,
            rot: (JITTER[d] ?? 0) * (a / 2.5),
            s: 1 - 0.02 * d,
            ox: 0.5,
            oy: 0.5,
          }),
  },
  pickup: {
    label: "Pick up",
    description: "The front cover lifts as the rest slide out.",
    rest: 5,
    fan: 12,
    spec: (d, a, fanned) =>
      d === 0
        ? fanned
          ? spec({ ty: -0.035, rot: -2, ox: 0.5, oy: 1 })
          : spec({ ox: 0.5, oy: 1 })
        : spec({
            tx: (d * a) / 100,
            rot: fanned ? d * 2.5 : d * 0.6,
            s: 1 - 0.04 * d,
            oy: 1,
          }),
  },
};

function corners(sp: Spec): Array<[number, number]> {
  const ox = sp.ox;
  const oy = sp.oy * H;
  const r = (sp.rot * Math.PI) / 180;
  const c = Math.cos(r);
  const s = Math.sin(r);
  return (
    [
      [0, 0],
      [1, 0],
      [1, H],
      [0, H],
    ] as const
  ).map(([x, y]) => {
    const px = (x - ox) * sp.s;
    const py = (y - oy) * sp.s;
    return [ox + sp.tx + c * px - s * py, oy + sp.ty * H + s * px + c * py];
  });
}

const n = (v: number, digits = 3) => Number(v.toFixed(digits));

/** `transform` for one state, pivot baked in (`transform-origin: 0 0`).
 *  Percent translates resolve against the cover's own box: x in cover
 *  widths, y in cover heights. */
function transformCss(sp: Spec): string {
  return (
    `translate(${n((sp.ox + sp.tx) * 100)}%, ${n((sp.oy + sp.ty) * 100)}%) ` +
    `rotate(${n(sp.rot)}deg) scale(${n(sp.s)}) ` +
    `translate(${n(-sp.ox * 100)}%, ${n(-sp.oy * 100)}%)`
  );
}

export type StackLayer = {
  /** 0 = front cover. */
  depth: number;
  rest: string;
  fan: string;
};

export type StackLayout = {
  /** Cover width as a % of the stack box width. */
  coverWidthPct: number;
  /** Cover box position inside the stack box, % of box width / height. */
  leftPct: number;
  topPct: number;
  /** Layers back-most first, so the front cover paints on top. */
  layers: StackLayer[];
};

type Fit = {
  specs: Array<{ depth: number; rest: Spec; fan: Spec }>;
  minX: number;
  maxX: number;
  minY: number;
  maxY: number;
};

function fit(v: Variant): Fit {
  const specs = Array.from({ length: BACK_COVERS + 1 }, (_, depth) => ({
    depth,
    rest: v.spec(depth, v.rest, false),
    fan: v.spec(depth, v.fan, true),
  }));
  let minX = 0;
  let maxX = 1;
  let minY = 0;
  let maxY = H;
  let restMinX = Infinity;
  let restMaxX = -Infinity;
  for (const sp of specs) {
    for (const st of [sp.rest, sp.fan]) {
      for (const [x, y] of corners(st)) {
        minX = Math.min(minX, x);
        maxX = Math.max(maxX, x);
        minY = Math.min(minY, y);
        maxY = Math.max(maxY, y);
        if (st === sp.rest) {
          restMinX = Math.min(restMinX, x);
          restMaxX = Math.max(restMaxX, x);
        }
      }
    }
  }
  // Centre the resting stack horizontally in the box: the box is sized for
  // the fanned spread, so an uncentred rest pile hugs the left edge with
  // the fan's empty runway beside it. Fanning then spreads out across the
  // whole box. The rest union is no wider than the full union, so the
  // shifted covers still fit.
  const shift = (minX + maxX) / 2 - (restMinX + restMaxX) / 2;
  for (const sp of specs) sp.rest = { ...sp.rest, tx: sp.rest.tx + shift };
  return { specs, minX, maxX, minY, maxY };
}

const FITS = Object.fromEntries(
  COVER_STACK_STYLES.map((k) => [k, fit(VARIANTS[k])]),
) as Record<CoverStackStyle, Fit>;

/** Stack box height / width, shared by every style (the tallest), so a
 *  style switch never reflows the card. */
export const STACK_ASPECT =
  Math.ceil(
    Math.max(
      ...COVER_STACK_STYLES.map((k) => {
        const f = FITS[k];
        return (f.maxY - f.minY) / (f.maxX - f.minX);
      }),
    ) * 1e4,
  ) / 1e4; // rounded up so the tallest style still fits

const LAYOUTS = Object.fromEntries(
  COVER_STACK_STYLES.map((k) => {
    const f = FITS[k];
    const span = f.maxX - f.minX; // stack width in cover widths
    const boxH = STACK_ASPECT * span; // box height in cover widths
    // Centre the style's own union vertically inside the shared box.
    const padY = (boxH - (f.maxY - f.minY)) / 2;
    const layout: StackLayout = {
      coverWidthPct: n(100 / span, 3),
      leftPct: n((-f.minX / span) * 100, 3),
      topPct: n(((padY - f.minY) / boxH) * 100, 3),
      layers: [...f.specs].reverse().map((sp) => ({
        depth: sp.depth,
        rest: transformCss(sp.rest),
        fan: transformCss(sp.fan),
      })),
    };
    return [k, layout];
  }),
) as Record<CoverStackStyle, StackLayout>;

export function layoutFor(style: CoverStackStyle): StackLayout {
  return LAYOUTS[style];
}

/** Exposed for tests: the raw union bounds in cover-width units. */
export function boundsFor(style: CoverStackStyle) {
  const { minX, maxX, minY, maxY } = FITS[style];
  return { minX, maxX, minY, maxY };
}

/** Exposed for tests: the corners of one cover in one state (rest
 *  includes the centring shift). */
export function cornersFor(
  style: CoverStackStyle,
  depth: number,
  fanned: boolean,
) {
  const sp = FITS[style].specs[depth];
  if (!sp) throw new Error(`no cover at depth ${depth}`);
  return corners(fanned ? sp.fan : sp.rest);
}
