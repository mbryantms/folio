"use client";

import * as React from "react";
import { Loader2 } from "lucide-react";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Slider } from "@/components/ui/slider";
import { applyChain, type RgbaImage } from "@/lib/image-transforms";
import type { TransformStep } from "@/lib/api/types";

/**
 * Per-page "Adjust…" panel for the archive page editor
 * (`archive-rewrite-1.0` M5). Sliders for brightness/contrast, levels,
 * sharpen and despeckle plus a drag-to-draw crop box, over a live canvas
 * preview.
 *
 * The preview loads the **full-resolution** page (`/issues/{id}/pages/{n}`)
 * so crop coordinates are real source pixels, then downscales to
 * {@link PREVIEW_MAX} for a responsive pixel preview (median/unsharp on a
 * full-res page would jank). Adjustments preview at that reduced scale —
 * algorithmic parity, not byte-exact — while the emitted `TransformStep[]`
 * carries crop in natural source pixels. Mirrors
 * `crates/server/src/jobs/archive_transforms.rs`.
 */

const PREVIEW_MAX = 512;

type CropPx = { x: number; y: number; w: number; h: number };

/** Pull initial slider state out of a stored chain (natural-px crop). */
function seedFromChain(chain: TransformStep[] | null) {
  let brightness = 0;
  let contrast = 0;
  let lo = 0;
  let hi = 255;
  let sharpen = 0;
  let despeckle = 0;
  let crop: CropPx | null = null;
  for (const s of chain ?? []) {
    if (s.kind === "brightness_contrast") {
      brightness = s.brightness;
      contrast = s.contrast;
    } else if (s.kind === "levels_clip") {
      lo = s.lo;
      hi = s.hi;
    } else if (s.kind === "sharpen") {
      sharpen = s.amount;
    } else if (s.kind === "despeckle") {
      despeckle = s.radius;
    } else if (s.kind === "crop_box") {
      crop = { x: s.x, y: s.y, w: s.w, h: s.h };
    }
  }
  return { brightness, contrast, lo, hi, sharpen, despeckle, crop };
}

export function PageAdjustDialog({
  issueId,
  orig,
  position,
  open,
  onOpenChange,
  initial,
  onApply,
}: {
  issueId: string;
  orig: number;
  position: number;
  open: boolean;
  onOpenChange: (next: boolean) => void;
  initial: TransformStep[] | null;
  onApply: (chain: TransformStep[] | null) => void;
}) {
  const [brightness, setBrightness] = React.useState(0);
  const [contrast, setContrast] = React.useState(0);
  const [lo, setLo] = React.useState(0);
  const [hi, setHi] = React.useState(255);
  const [sharpen, setSharpen] = React.useState(0);
  const [despeckle, setDespeckle] = React.useState(0);
  const [crop, setCrop] = React.useState<CropPx | null>(null);

  const canvasRef = React.useRef<HTMLCanvasElement | null>(null);
  const overlayRef = React.useRef<HTMLDivElement | null>(null);
  const baseRef = React.useRef<RgbaImage | null>(null);
  const [natural, setNatural] = React.useState<{ w: number; h: number } | null>(
    null,
  );
  const [loading, setLoading] = React.useState(true);
  const [loadError, setLoadError] = React.useState(false);
  const [drag, setDrag] = React.useState<{ x0: number; y0: number } | null>(
    null,
  );

  // Seed slider state + load the page each time the panel opens.
  const [wasOpen, setWasOpen] = React.useState(false);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) {
      const s = seedFromChain(initial);
      setBrightness(s.brightness);
      setContrast(s.contrast);
      setLo(s.lo);
      setHi(s.hi);
      setSharpen(s.sharpen);
      setDespeckle(s.despeckle);
      setCrop(s.crop);
    }
  }

  // Reset load state during render whenever the panel opens or the target
  // page changes — the same render-phase idiom as the slider-seed block
  // above, and the documented alternative to a setState-in-effect reset
  // (https://react.dev/learn/you-might-not-need-an-effect). Keeps the
  // effect body free of synchronous setState.
  const loadKey = open ? `${issueId}:${orig}` : "";
  const [lastLoadKey, setLastLoadKey] = React.useState("");
  if (loadKey !== lastLoadKey) {
    setLastLoadKey(loadKey);
    if (open) {
      setLoading(true);
      setLoadError(false);
    }
  }

  React.useEffect(() => {
    if (!open) return;
    baseRef.current = null;
    const img = new window.Image();
    img.onload = () => {
      const nw = img.naturalWidth;
      const nh = img.naturalHeight;
      const scale = Math.min(1, PREVIEW_MAX / Math.max(nw, nh));
      const pw = Math.max(1, Math.round(nw * scale));
      const ph = Math.max(1, Math.round(nh * scale));
      const off = document.createElement("canvas");
      off.width = pw;
      off.height = ph;
      const octx = off.getContext("2d");
      if (!octx) {
        setLoadError(true);
        setLoading(false);
        return;
      }
      octx.drawImage(img, 0, 0, pw, ph);
      baseRef.current = octx.getImageData(0, 0, pw, ph);
      setNatural({ w: nw, h: nh });
      setLoading(false);
    };
    img.onerror = () => {
      setLoadError(true);
      setLoading(false);
    };
    img.src = `/issues/${issueId}/pages/${orig}`;
    return () => {
      img.onload = null;
      img.onerror = null;
    };
  }, [open, issueId, orig]);

  // Re-render the preview whenever an adjustment changes. Crop is drawn as
  // an overlay box, not applied to the canvas, so the box stays editable.
  React.useEffect(() => {
    const base = baseRef.current;
    const canvas = canvasRef.current;
    if (!base || !canvas) return;
    const chain: TransformStep[] = [];
    if (brightness !== 0 || contrast !== 0)
      chain.push({ kind: "brightness_contrast", brightness, contrast });
    if (lo < hi && (lo > 0 || hi < 255))
      chain.push({ kind: "levels_clip", lo, hi });
    if (sharpen > 0) chain.push({ kind: "sharpen", amount: sharpen });
    if (despeckle > 0) chain.push({ kind: "despeckle", radius: despeckle });
    const out = applyChain(base, chain);
    canvas.width = out.width;
    canvas.height = out.height;
    const ctx = canvas.getContext("2d");
    if (ctx) {
      // Build via dimensions + `.set()` rather than the
      // `new ImageData(data, w, h)` overload: under the project's
      // `lib` the constructor types `data` as `Uint8ClampedArray<
      // ArrayBuffer>`, which our `RgbaImage.data` (`ArrayBufferLike`)
      // doesn't satisfy. Copying into a fresh buffer is equivalent and
      // type-clean.
      const imageData = ctx.createImageData(out.width, out.height);
      imageData.data.set(out.data);
      ctx.putImageData(imageData, 0, 0);
    }
  }, [brightness, contrast, lo, hi, sharpen, despeckle, loading]);

  function pointerFraction(e: React.PointerEvent) {
    const rect = overlayRef.current?.getBoundingClientRect();
    if (!rect) return null;
    return {
      fx: Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width)),
      fy: Math.min(1, Math.max(0, (e.clientY - rect.top) / rect.height)),
    };
  }

  function onPointerDown(e: React.PointerEvent) {
    const p = pointerFraction(e);
    if (!p) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    setDrag({ x0: p.fx, y0: p.fy });
  }

  function onPointerMove(e: React.PointerEvent) {
    if (!drag || !natural) return;
    const p = pointerFraction(e);
    if (!p) return;
    const fx = Math.min(drag.x0, p.fx);
    const fy = Math.min(drag.y0, p.fy);
    const fw = Math.abs(p.fx - drag.x0);
    const fh = Math.abs(p.fy - drag.y0);
    setCrop({
      x: Math.round(fx * natural.w),
      y: Math.round(fy * natural.h),
      w: Math.round(fw * natural.w),
      h: Math.round(fh * natural.h),
    });
  }

  function onPointerUp() {
    if (crop && (crop.w < 4 || crop.h < 4)) setCrop(null); // ignore taps
    setDrag(null);
  }

  function reset() {
    setBrightness(0);
    setContrast(0);
    setLo(0);
    setHi(255);
    setSharpen(0);
    setDespeckle(0);
    setCrop(null);
  }

  function buildChain(): TransformStep[] {
    const chain: TransformStep[] = [];
    if (brightness !== 0 || contrast !== 0)
      chain.push({ kind: "brightness_contrast", brightness, contrast });
    if (lo < hi && (lo > 0 || hi < 255))
      chain.push({ kind: "levels_clip", lo, hi });
    if (sharpen > 0) chain.push({ kind: "sharpen", amount: sharpen });
    if (despeckle > 0) chain.push({ kind: "despeckle", radius: despeckle });
    if (crop && crop.w > 0 && crop.h > 0)
      chain.push({ kind: "crop_box", ...crop });
    return chain;
  }

  function apply() {
    const chain = buildChain();
    onApply(chain.length > 0 ? chain : null);
    onOpenChange(false);
  }

  const cropStyle: React.CSSProperties | null =
    crop && natural
      ? {
          left: `${(crop.x / natural.w) * 100}%`,
          top: `${(crop.y / natural.h) * 100}%`,
          width: `${(crop.w / natural.w) * 100}%`,
          height: `${(crop.h / natural.h) * 100}%`,
        }
      : null;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[90vh] w-full flex-col gap-0 p-0 sm:max-w-3xl">
        <DialogHeader className="border-border border-b px-6 py-4">
          <DialogTitle>Adjust page {position}</DialogTitle>
          <DialogDescription>
            Tune the image and drag on the preview to set a crop box. Changes
            apply when you rewrite the archive.
          </DialogDescription>
        </DialogHeader>

        <div className="grid flex-1 gap-6 overflow-y-auto px-6 py-4 md:grid-cols-[1fr_18rem]">
          <div className="bg-muted relative grid place-items-center rounded-md">
            {loading ? (
              <div className="text-muted-foreground flex items-center gap-2 py-12 text-sm">
                <Loader2 className="h-4 w-4 animate-spin" /> Loading page…
              </div>
            ) : loadError ? (
              <p className="text-destructive py-12 text-sm">
                Couldn&rsquo;t load the page image.
              </p>
            ) : (
              <div className="relative inline-block max-h-[60vh]">
                <canvas
                  ref={canvasRef}
                  className="max-h-[60vh] w-auto rounded-md"
                />
                <div
                  ref={overlayRef}
                  className="absolute inset-0 cursor-crosshair touch-none"
                  onPointerDown={onPointerDown}
                  onPointerMove={onPointerMove}
                  onPointerUp={onPointerUp}
                >
                  {cropStyle && (
                    <div
                      className="border-primary bg-primary/10 pointer-events-none absolute border-2"
                      style={cropStyle}
                    />
                  )}
                </div>
              </div>
            )}
          </div>

          <div className="space-y-4">
            <AdjustSlider
              label="Brightness"
              value={brightness}
              min={-100}
              max={100}
              onChange={setBrightness}
            />
            <AdjustSlider
              label="Contrast"
              value={contrast}
              min={-100}
              max={100}
              onChange={setContrast}
            />
            <div className="space-y-1.5">
              <div className="flex items-center justify-between">
                <Label className="text-xs">Levels (black / white point)</Label>
                <span className="text-muted-foreground text-xs tabular-nums">
                  {lo} / {hi}
                </span>
              </div>
              <Slider
                value={[lo, hi]}
                min={0}
                max={255}
                step={1}
                onValueChange={([a, b]) => {
                  setLo(Math.min(a, b));
                  setHi(Math.max(a, b));
                }}
              />
            </div>
            <AdjustSlider
              label="Sharpen"
              value={sharpen}
              min={0}
              max={5}
              step={0.1}
              onChange={setSharpen}
            />
            <AdjustSlider
              label="Despeckle"
              value={despeckle}
              min={0}
              max={4}
              step={1}
              onChange={setDespeckle}
            />
            <div className="flex items-center justify-between">
              <span className="text-muted-foreground text-xs">
                {crop ? `Crop ${crop.w}×${crop.h}px` : "No crop"}
              </span>
              {crop && (
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="h-7 text-xs"
                  onClick={() => setCrop(null)}
                >
                  Clear crop
                </Button>
              )}
            </div>
          </div>
        </div>

        <DialogFooter className="border-border flex items-center justify-between gap-2 border-t px-6 py-4 sm:justify-between">
          <Button type="button" variant="ghost" onClick={reset}>
            Reset
          </Button>
          <div className="flex items-center gap-2">
            <Button
              type="button"
              variant="ghost"
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <Button type="button" onClick={apply} disabled={loadError}>
              Apply
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function AdjustSlider({
  label,
  value,
  min,
  max,
  step = 1,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  onChange: (v: number) => void;
}) {
  return (
    <div className="space-y-1.5">
      <div className="flex items-center justify-between">
        <Label className="text-xs">{label}</Label>
        <span className="text-muted-foreground text-xs tabular-nums">
          {value}
        </span>
      </div>
      <Slider
        value={[value]}
        min={min}
        max={max}
        step={step}
        onValueChange={([v]) => onChange(v)}
      />
    </div>
  );
}
