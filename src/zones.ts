import { zoneIndex, type ZoneDefinition } from "./types";

/**
 * Shared zone palette. Index N is zone N+1; lists longer than the palette wrap.
 * Every chart, editor swatch, and workout shape that colours by zone must go
 * through `zoneColorAt` or `zoneColor` so they agree.
 */
export const zoneColors: readonly string[] = [
  "#6ca8ff",
  "#63d6c6",
  "#c8ff32",
  "#f4d35e",
  "#ff9f43",
  "#ff6f7d",
  "#c77dff",
  "#9d6b53",
  "#d0d5ce",
  "#ffffff",
];

export const zoneColorAt = (index: number): string =>
  zoneColors[((index % zoneColors.length) + zoneColors.length) % zoneColors.length];

/**
 * Colour for a value against a zone list. Values above every bounded zone in
 * a list with no open-ended zone fall into the last zone.
 */
export const zoneColor = (value: number, zones: readonly ZoneDefinition[]): string => {
  if (zones.length === 0) return zoneColorAt(0);
  const index = zoneIndex(value, zones);
  return zoneColorAt(index < 0 ? zones.length - 1 : index);
};
