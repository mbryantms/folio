import devices from "./apple-splash-devices.json";

/**
 * `apple-touch-startup-image` links for iOS/iPadOS standalone launches.
 *
 * The device list in `apple-splash-devices.json` is shared with
 * `scripts/build-icons.mjs`, which renders one portrait and one landscape
 * PNG per device at `/icons/splash-<pixel width>x<pixel height>.png`. The
 * `media` query binds a file to an exact CSS viewport, pixel ratio, and
 * orientation; iOS picks the first match and falls back to a plain
 * `background_color` splash for devices not listed here.
 */
export type AppleStartupImage = {
  rel: "apple-touch-startup-image";
  url: string;
  media: string;
};

export function appleStartupImages(): AppleStartupImage[] {
  return devices.flatMap(({ width, height, ratio }) =>
    (["portrait", "landscape"] as const).map((orientation) => {
      const [w, h] =
        orientation === "portrait"
          ? [width * ratio, height * ratio]
          : [height * ratio, width * ratio];
      return {
        rel: "apple-touch-startup-image" as const,
        url: `/icons/splash-${w}x${h}.png`,
        media: `(device-width: ${width}px) and (device-height: ${height}px) and (-webkit-device-pixel-ratio: ${ratio}) and (orientation: ${orientation})`,
      };
    }),
  );
}
