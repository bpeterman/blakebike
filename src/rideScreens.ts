/**
 * The rider's ride-screen layout: several screens, each an ordered list of
 * field slots and panels. Persisted as one document (`ride_display_preferences`);
 * `RideDisplayPreferences` in `storage.rs` mirrors this shape and the same
 * normalization rules.
 */
import {
  isRideField,
  isRidePanelId,
  rideFieldKey,
  type RideField,
  type RidePanelId,
} from "./rideFields";

/** Columns of the four-column ride grid a slot takes up. */
export const rideSlotSpans = [1, 2, 4] as const;

export type RideSlotSpan = (typeof rideSlotSpans)[number];

export type RideScreenItem =
  | { kind: "field"; field: RideField; span: RideSlotSpan }
  | { kind: "panel"; panel: RidePanelId };

export type RideScreen = {
  id: string;
  name: string;
  items: RideScreenItem[];
};

export type RideDisplayPreferences = {
  version: 3;
  screens: RideScreen[];
};

/** More than this many screens is a corrupt document, not a layout. */
export const MAX_RIDE_SCREENS = 8;

export const MAX_RIDE_SCREEN_ITEMS = 24;

const field = (
  metric: RideField["metric"],
  aggregate: RideField["aggregate"] = "current",
  scope: RideField["scope"] = "ride",
  span: RideSlotSpan = 1,
): RideScreenItem => ({ kind: "field", field: { metric, scope, aggregate }, span });

const panel = (id: RidePanelId): RideScreenItem => ({ kind: "panel", panel: id });

/**
 * What a rider sees before they arrange anything themselves: the numbers they
 * ride by on the first screen, the ones they check on the second.
 */
export const defaultRideDisplayPreferences: RideDisplayPreferences = {
  version: 3,
  screens: [
    {
      id: "ride",
      name: "Ride",
      items: [
        field("power", "current", "ride", 2),
        field("cadence"),
        field("heartRate"),
        field("speed"),
        field("power", "average"),
        field("wattsPerKilogram"),
        field("distance", "total"),
        field("time", "total"),
        field("time", "remaining"),
        panel("workoutTimeline"),
        panel("targetAndBias"),
        panel("powerChart"),
      ],
    },
    {
      id: "detail",
      name: "Detail",
      items: [
        field("energy", "total"),
        field("calories", "total"),
        field("power", "average", "interval"),
        field("heartRate", "average"),
        panel("heartRateChart"),
        panel("timeInZone"),
      ],
    },
  ],
};

const isSpan = (value: unknown): value is RideSlotSpan =>
  (rideSlotSpans as readonly unknown[]).includes(value);

/** Identity of a slot, so a screen cannot hold the same thing twice. */
const itemKey = (item: RideScreenItem): string =>
  item.kind === "field" ? `field:${rideFieldKey(item.field)}` : `panel:${item.panel}`;

const normalizeItems = (items: readonly RideScreenItem[]): RideScreenItem[] => {
  const seen = new Set<string>();
  const result: RideScreenItem[] = [];
  for (const item of items) {
    if (result.length === MAX_RIDE_SCREEN_ITEMS) break;
    if (item.kind === "field") {
      if (!isRideField(item.field)) continue;
    } else if (!isRidePanelId(item.panel)) {
      continue;
    }
    const key = itemKey(item);
    if (seen.has(key)) continue;
    seen.add(key);
    result.push(
      item.kind === "field"
        ? { kind: "field", field: item.field, span: isSpan(item.span) ? item.span : 1 }
        : item,
    );
  }
  return result;
};

/**
 * Drops fields and panels this version does not know, screens left with
 * nothing on them, and anything past the caps — then guarantees the rider is
 * never left without a screen to ride on.
 */
export const normalizeRideDisplayPreferences = (
  preferences: RideDisplayPreferences,
): RideDisplayPreferences => {
  const ids = new Set<string>();
  const screens: RideScreen[] = [];
  for (const screen of preferences.screens) {
    if (screens.length === MAX_RIDE_SCREENS) break;
    const items = normalizeItems(screen.items);
    if (items.length === 0) continue;
    let id = screen.id.trim() === "" ? `screen-${screens.length + 1}` : screen.id;
    while (ids.has(id)) id = `${id}-${screens.length + 1}`;
    ids.add(id);
    screens.push({
      id,
      name: screen.name.trim() === "" ? `Screen ${screens.length + 1}` : screen.name,
      items,
    });
  }
  return {
    version: 3,
    screens: screens.length > 0 ? screens : defaultRideDisplayPreferences.screens,
  };
};

/** The v2 cards, each as the field or panel that replaced it. */
const v2Items: Record<string, RideScreenItem> = {
  power: field("power", "current", "ride", 2),
  cadence: field("cadence"),
  speed: field("speed"),
  heartRate: field("heartRate"),
  energy: field("energy", "total"),
  averagePower: field("power", "average"),
  wattsPerKilogram: field("wattsPerKilogram"),
  distance: field("distance", "total"),
  elapsedTime: field("time", "total"),
  remainingTime: field("time", "remaining"),
  workoutTimeline: panel("workoutTimeline"),
  targetAndBias: panel("targetAndBias"),
  powerChart: panel("powerChart"),
  heartRateChart: panel("heartRateChart"),
  timeInZone: panel("timeInZone"),
  deviceStats: panel("deviceStats"),
};

type V2Preferences = {
  version: number;
  cards: Array<{ id: string; visible: boolean }>;
};

const isV2 = (value: unknown): value is V2Preferences =>
  typeof value === "object" &&
  value !== null &&
  Array.isArray((value as V2Preferences).cards);

const isV3 = (value: unknown): value is RideDisplayPreferences =>
  typeof value === "object" &&
  value !== null &&
  Array.isArray((value as RideDisplayPreferences).screens);

/**
 * Reads whatever was stored. A layout from before ride screens becomes one
 * screen holding the cards the rider had switched on, in their order, so
 * nobody has to rebuild a layout they already chose.
 */
export const migrateRideDisplayPreferences = (
  stored: unknown,
): RideDisplayPreferences => {
  if (isV3(stored)) return normalizeRideDisplayPreferences(stored);
  if (isV2(stored)) {
    const items = stored.cards
      .filter((card) => card.visible)
      .map((card) => v2Items[card.id])
      .filter((item): item is RideScreenItem => item !== undefined);
    return normalizeRideDisplayPreferences({
      version: 3,
      screens: [{ id: "ride", name: "Ride", items }],
    });
  }
  return defaultRideDisplayPreferences;
};
