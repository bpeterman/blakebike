import { describe, expect, it } from "vitest";
import { derivedPowerZones, type ZoneDefinition } from "./types";
import { zoneColor, zoneColorAt, zoneColors } from "./zones";

describe("zone colours", () => {
  it("wraps indexes past the palette and tolerates negatives", () => {
    expect(zoneColorAt(0)).toBe(zoneColors[0]);
    expect(zoneColorAt(zoneColors.length)).toBe(zoneColors[0]);
    expect(zoneColorAt(zoneColors.length + 2)).toBe(zoneColors[2]);
    expect(zoneColorAt(-1)).toBe(zoneColors[zoneColors.length - 1]);
  });

  it("colours a value by the zone it falls in, inclusive of the upper bound", () => {
    const zones = derivedPowerZones(200);
    expect(zoneColor(110, zones)).toBe(zoneColors[0]);
    expect(zoneColor(111, zones)).toBe(zoneColors[1]);
    expect(zoneColor(500, zones)).toBe(zoneColors[6]);
  });

  it("uses the same indexing for short custom zone lists", () => {
    const zones: ZoneDefinition[] = [
      { name: "Easy", upperBound: 150 },
      { name: "Hard", upperBound: 250 },
      { name: "Max", upperBound: null },
    ];
    expect(zoneColor(100, zones)).toBe(zoneColors[0]);
    expect(zoneColor(200, zones)).toBe(zoneColors[1]);
    expect(zoneColor(999, zones)).toBe(zoneColors[2]);
  });

  it("falls back to the last zone when no zone is open-ended", () => {
    const zones: ZoneDefinition[] = [
      { name: "A", upperBound: 100 },
      { name: "B", upperBound: 200 },
    ];
    expect(zoneColor(300, zones)).toBe(zoneColors[1]);
    expect(zoneColor(300, [])).toBe(zoneColors[0]);
  });
});
