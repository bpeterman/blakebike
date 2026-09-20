import { describe, expect, it } from "vitest";
// The Rust field table, read as text so the two languages can be compared.
import storageSource from "../src-tauri/src/storage.rs?raw";
import {
  isRideField,
  rideAggregatesFor,
  rideFieldKey,
  rideFieldLabel,
  rideFields,
  ridePanelIds,
  rideScopesFor,
  resolveRideField,
  type RideFieldContext,
} from "./rideFields";
import type { Profile, Telemetry } from "./types";

const profile: Profile = {
  id: "rider",
  name: "Rider",
  ftpWatts: 200,
  maxPowerWatts: 1000,
  maxHeartRateBpm: 190,
  riderWeightKg: 75,
  bikeWeightKg: 9,
  weightUnit: "kg",
  distanceUnit: "km",
};

const sample = (second: number, powerWatts: number, heartRateBpm: number | null = 140): Telemetry => ({
  timestampMs: second * 1000,
  powerWatts,
  cadenceRpm: 90,
  speedKph: 30,
  heartRateBpm,
  targetPowerWatts: 200,
});

// Two minutes: 200 W for the first, 300 W for the second. The last 30 s are
// the block being ridden.
const history = Array.from({ length: 121 }, (_, second) =>
  sample(second, second < 60 ? 200 : 300),
);

const context = (overrides: Partial<RideFieldContext> = {}): RideFieldContext => ({
  telemetry: sample(120, 300),
  history,
  intervalHistory: history.slice(90),
  displayPowerWatts: 260,
  elapsedSeconds: 120,
  totalSeconds: 600,
  intervalElapsedSeconds: 30,
  intervalTotalSeconds: 120,
  targetPowerWatts: 250,
  distanceMeters: 5_000,
  profile,
  ...overrides,
});

describe("ride fields", () => {
  it("only offers combinations it can resolve", () => {
    expect(isRideField({ metric: "power", scope: "ride", aggregate: "average" })).toBe(true);
    // A live reading is the same number whatever the scope, so it is offered once.
    expect(isRideField({ metric: "power", scope: "interval", aggregate: "current" })).toBe(false);
    // Distance is only totalled over the whole ride.
    expect(isRideField({ metric: "distance", scope: "interval", aggregate: "total" })).toBe(false);
    expect(isRideField({ metric: "heartRate", scope: "ride", aggregate: "total" })).toBe(false);
    expect(rideFieldLabel({ metric: "nonsense", scope: "ride", aggregate: "current" } as never)).toBeNull();
    expect(rideFields.every((field) => rideFieldLabel(field) !== null)).toBe(true);
  });

  it("names a field by what it measures, over what, and how", () => {
    expect(rideFieldLabel({ metric: "power", scope: "ride", aggregate: "current" })).toBe("POWER");
    expect(rideFieldLabel({ metric: "power", scope: "ride", aggregate: "average" })).toBe("AVG POWER");
    expect(rideFieldLabel({ metric: "power", scope: "interval", aggregate: "max" })).toBe("BLOCK MAX POWER");
    expect(rideFieldLabel({ metric: "time", scope: "interval", aggregate: "remaining" })).toBe("BLOCK LEFT");
  });

  it("resolves live readings, averages and peaks over the right samples", () => {
    const resolved = (metric: string, scope = "ride", aggregate = "current") =>
      resolveRideField({ metric, scope, aggregate } as never, context());

    // The live power tile shows the smoothed figure, not the raw sample.
    expect(resolved("power")).toEqual({ label: "POWER", value: "260", unit: "W" });
    expect(resolved("power", "ride", "average")).toEqual({
      label: "AVG POWER",
      value: "250",
      unit: "W",
    });
    // The block has only been at 300 W.
    expect(resolved("power", "interval", "average")).toEqual({
      label: "BLOCK AVG POWER",
      value: "300",
      unit: "W",
    });
    expect(resolved("power", "ride", "max")).toEqual({ label: "MAX POWER", value: "300", unit: "W" });
    expect(resolved("wattsPerKilogram")).toEqual({ label: "W/KG", value: "3.47", unit: "W/kg" });
    expect(resolved("distance", "ride", "total")).toEqual({
      label: "DISTANCE",
      value: "5.00",
      unit: "km",
    });
    expect(resolved("energy", "ride", "total")).toEqual({ label: "ENERGY", value: "30", unit: "kJ" });
    expect(resolved("calories", "ride", "total")).toEqual({
      label: "CALORIES",
      value: "30",
      unit: "Cal",
    });
    expect(resolved("time", "ride", "total")).toEqual({ label: "ELAPSED", value: "2:00" });
    expect(resolved("time", "ride", "remaining")).toEqual({ label: "REMAINING", value: "8:00" });
    expect(resolved("time", "interval", "remaining")).toEqual({ label: "BLOCK LEFT", value: "1:30" });
    expect(resolved("targetPower")).toEqual({ label: "TARGET", value: "250", unit: "W" });
  });

  it("holds its place when a reading is missing, and stands down when the ride has no end", () => {
    const noHeartRate = context({
      telemetry: { ...sample(120, 300), heartRateBpm: null },
      history: history.map((entry) => ({ ...entry, heartRateBpm: null })),
    });
    expect(resolveRideField({ metric: "heartRate", scope: "ride", aggregate: "current" }, noHeartRate))
      .toEqual({ label: "HEART RATE", value: "—", unit: "bpm" });
    expect(resolveRideField({ metric: "heartRate", scope: "ride", aggregate: "average" }, noHeartRate))
      .toEqual({ label: "AVG HEART RATE", value: "—", unit: "bpm" });

    // A free ride, and a free-ride block, have no time left in them.
    const openEnded = context({ totalSeconds: null, intervalTotalSeconds: null });
    expect(resolveRideField({ metric: "time", scope: "ride", aggregate: "remaining" }, openEnded)).toBeNull();
    expect(
      resolveRideField({ metric: "time", scope: "interval", aggregate: "remaining" }, openEnded),
    ).toBeNull();
    // Elapsed still works on a free ride.
    expect(resolveRideField({ metric: "time", scope: "ride", aggregate: "total" }, openEnded)).toEqual({
      label: "ELAPSED",
      value: "2:00",
    });
    expect(resolveRideField({ metric: "targetPower", scope: "ride", aggregate: "current" }, context({ targetPowerWatts: null })))
      .toEqual({ label: "TARGET", value: "Free" });
  });

  it("holds exactly the fields and panels the Rust side accepts", () => {
    // The two lists are written twice, once per language. This is what stops
    // them drifting: a field only one side knows is dropped on load.
    const listOf = (name: string): string[] => {
      const match = storageSource.match(
        new RegExp(`const ${name}: \\[&str; \\d+\\] = \\[([^\\]]*)\\]`),
      );
      return (match?.[1].match(/"([^"]+)"/g) ?? []).map((entry: string) => entry.slice(1, -1));
    };
    expect(new Set(listOf("RIDE_FIELDS"))).toEqual(new Set(rideFields.map(rideFieldKey)));
    expect(listOf("RIDE_PANELS")).toEqual([...ridePanelIds]);
  });

  it("offers each metric the scopes and aggregates it supports", () => {
    expect(rideScopesFor("power")).toEqual(["ride", "interval"]);
    expect(rideScopesFor("distance")).toEqual(["ride"]);
    expect(rideAggregatesFor("power", "ride")).toEqual(["current", "average", "max"]);
    expect(rideAggregatesFor("power", "interval")).toEqual(["average", "max"]);
    expect(rideAggregatesFor("wattsPerKilogram", "interval")).toEqual(["average"]);
    expect(rideAggregatesFor("time", "ride")).toEqual(["total", "remaining"]);
  });
});
