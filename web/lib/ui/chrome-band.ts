/** The one translucent-chrome treatment for every band that floats
 *  over scrolling content: the topbar, the bottom tab bar, the sticky
 *  phone controls row, the selection toolbar and the desktop library
 *  toolbar once it is held. Sharing the string keeps the tint and blur
 *  identical across them — the sticky rows used to sit at 95% while
 *  the topbar was 80%, so the two bands read as different materials
 *  when stacked. Standalone (installed PWA) drops the blur: iOS
 *  composites `backdrop-filter` poorly under its own status bar. */
export const CHROME_BAND_CLASS =
  "border-border bg-background/80 backdrop-blur standalone:bg-background standalone:backdrop-blur-none";
