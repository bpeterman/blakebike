import { describe, expect, it } from "vitest";
// The Rust default layout, read as text so the two languages can be compared.
import storageSource from "../src-tauri/src/storage.rs?raw";
import { rideFieldKey } from "./rideFields";
import {
  MAX_RIDE_SCREENS,
  defaultRideDisplayPreferences,
  migrateRideDisplayPreferences,
  normalizeRideDisplayPreferences,
  type RideScreenItem,
} from "./rideScreens";

const field = (
  metric: string,
  scope = "ride",
  aggregate = "current",
  span = 1,
): RideScreenItem =>
  ({
    kind: "field",
    field: { metric, scope, aggregate },
    span,
  }) as RideScreenItem;

describe("ride screens", () => {
  it("ships screens a rider can ride with out of the box", () => {
    expect(defaultRideDisplayPreferences.screens.map((screen) => screen.name)).toEqual([
      "Ride",
      "Detail",
    ]);
    expect(normalizeRideDisplayPreferences(defaultRideDisplayPreferences)).toEqual(
      defaultRideDisplayPreferences,
    );
    // Stats for nerds is still opt-in: it is on no default screen.
    expect(
      defaultRideDisplayPreferences.screens.some((screen) =>
        screen.items.some((item) => item.kind === "panel" && item.panel === "deviceStats"),
      ),
    ).toBe(false);
  });

  it("ships the same default layout as the Rust side", () => {
    // Each screen as `field:key:span` / `panel:id` lines, in order.
    const body = storageSource.match(/fn default_screens\(\)[\s\S]*?\n}\n/)?.[0] ?? "";
    const rust = [...body.matchAll(/items: vec!\[([\s\S]*?)\]/g)].map(([, items]) =>
      [...items.matchAll(/field\("([^"]+)", (\d)\)|panel\("([^"]+)"\)/g)].map(
        ([, key, span, panel]) => (panel === undefined ? `field:${key}:${span}` : `panel:${panel}`),
      ),
    );
    expect(rust).toEqual(
      defaultRideDisplayPreferences.screens.map((screen) =>
        screen.items.map((item) =>
          item.kind === "field"
            ? `field:${rideFieldKey(item.field)}:${item.span}`
            : `panel:${item.panel}`,
        ),
      ),
    );
  });

  it("leaves a saved layout as the rider built it, %FTP or not", () => {
    // New default fields reach riders who never arranged their screens, and
    // nobody else: a stored layout is read back as stored.
    const before = {
      version: 3,
      screens: [{ id: "mine", name: "Mine", items: [field("power"), field("cadence")] }],
    };
    expect(migrateRideDisplayPreferences(before).screens[0].items).toEqual([
      field("power"),
      field("cadence"),
    ]);
    const withPercent = {
      version: 3,
      screens: [
        {
          id: "mine",
          name: "Mine",
          items: [field("ftpPercent", "interval", "average", 2), field("targetFtpPercent")],
        },
      ],
    };
    expect(migrateRideDisplayPreferences(withPercent)).toEqual(withPercent);
  });

  it("drops what it cannot show and never leaves the rider screenless", () => {
    const normalized = normalizeRideDisplayPreferences({
      version: 3,
      screens: [
        {
          id: "",
          name: "",
          items: [
            field("power", "ride", "current", 3),
            field("power", "ride", "current"),
            field("vo2max"),
            { kind: "panel", panel: "spaceship" } as unknown as RideScreenItem,
          ],
        },
        { id: "empty", name: "Nothing here", items: [] },
      ],
    });
    expect(normalized.screens).toHaveLength(1);
    expect(normalized.screens[0].id).toBe("screen-1");
    expect(normalized.screens[0].name).toBe("Screen 1");
    // The duplicate is dropped and the impossible span falls back to small.
    expect(normalized.screens[0].items).toEqual([field("power", "ride", "current", 1)]);

    const emptied = normalizeRideDisplayPreferences({ version: 3, screens: [] });
    expect(emptied.screens).toEqual(defaultRideDisplayPreferences.screens);
  });

  it("caps runaway documents and keeps screen ids unique", () => {
    const screens = Array.from({ length: MAX_RIDE_SCREENS + 3 }, () => ({
      id: "same",
      name: "Screen",
      items: [field("power")],
    }));
    const normalized = normalizeRideDisplayPreferences({ version: 3, screens });
    expect(normalized.screens).toHaveLength(MAX_RIDE_SCREENS);
    expect(new Set(normalized.screens.map((screen) => screen.id)).size).toBe(MAX_RIDE_SCREENS);
  });

  it("turns a saved card layout into one screen, keeping the rider's order", () => {
    const migrated = migrateRideDisplayPreferences({
      version: 2,
      cards: [
        { id: "speed", visible: true },
        { id: "power", visible: true },
        { id: "deviceStats", visible: false },
        { id: "timeInZone", visible: true },
        { id: "nonsense", visible: true },
      ],
    });
    expect(migrated.screens).toHaveLength(1);
    expect(migrated.screens[0].name).toBe("Ride");
    expect(migrated.screens[0].items).toEqual([
      field("speed"),
      // Power keeps the double width it has on the ride screen.
      field("power", "ride", "current", 2),
      { kind: "panel", panel: "timeInZone" },
    ]);
  });

  it("falls back to the defaults for anything it cannot read", () => {
    expect(migrateRideDisplayPreferences({ showTimeInZone: false })).toEqual(
      defaultRideDisplayPreferences,
    );
    expect(migrateRideDisplayPreferences(null)).toEqual(defaultRideDisplayPreferences);
    // A layout where every card was hidden still leaves somewhere to ride.
    expect(
      migrateRideDisplayPreferences({ version: 2, cards: [{ id: "power", visible: false }] }),
    ).toEqual(defaultRideDisplayPreferences);
  });

  it("passes a v3 document through, normalizing it", () => {
    const stored = {
      version: 3,
      screens: [{ id: "one", name: "One", items: [field("cadence"), field("cadence")] }],
    };
    expect(migrateRideDisplayPreferences(stored).screens[0].items).toEqual([field("cadence")]);
  });
});
