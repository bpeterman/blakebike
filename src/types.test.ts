import { describe, expect, it } from "vitest";
import {
  biasedWatts,
  compileWorkoutIntervals,
  formatDistance,
  formatDuration,
  formatIntervalTarget,
  formatSpeed,
  derivedHeartRateZones,
  derivedPowerZones,
  defaultTrainingZoneSettings,
  describeTrainingSync,
  downsampleTelemetry,
  effectiveHeartRateZones,
  effectivePowerZones,
  manualPowerDeltaForKey,
  rideKeyAction,
  clampBias,
  withSmoothedPower,
  timeInZones,
  withActiveElapsed,
  zoneIndex,
  workoutDuration,
  zoneModeLabel,
  workKilojoules,
  averagePowerWatts,
  kilojoulesToKilocalories,
  type TrainingSyncResult,
  type WorkoutStep,
} from "./types";

describe("workout helpers", () => {
  it("scales targets by bias exactly like the Rust runner", () => {
    expect(biasedWatts(200, 100)).toBe(200);
    expect(biasedWatts(200, 105)).toBe(210);
    expect(biasedWatts(201, 95)).toBe(191); // 190.95 rounds up
    expect(biasedWatts(1, 50)).toBe(1); // never zero
    expect(biasedWatts(0, 100)).toBe(1); // ERG treats 0 W as "no target"
    expect(biasedWatts(200, 500)).toBe(300); // clamped to MAX_BIAS_PERCENT
    expect(biasedWatts(200, 10)).toBe(100); // clamped to MIN_BIAS_PERCENT
  });

  it("compiles intervals with the ride's bias applied to every target", () => {
    const steps: WorkoutStep[] = [
      { kind: "steady", durationSeconds: 60, target: { unit: "percentFtp", value: 75 } },
      {
        kind: "ramp",
        durationSeconds: 30,
        start: { unit: "watts", value: 100 },
        end: { unit: "percentFtp", value: 125 },
      },
      { kind: "freeRide", durationSeconds: 15 },
    ];
    expect(compileWorkoutIntervals(steps, 200, 110).map(({ startWatts, endWatts }) => [startWatts, endWatts])).toEqual([
      [165, 165],
      [110, 275],
      [null, null],
    ]);
    expect(compileWorkoutIntervals(steps, 200, 100)).toEqual(compileWorkoutIntervals(steps, 200));
  });

  it("formats interval targets for labels and headings", () => {
    expect(formatIntervalTarget(null, null)).toBe("Free ride");
    expect(formatIntervalTarget(200, 200)).toBe("200 W");
    expect(formatIntervalTarget(150, 220)).toBe("150–220 W");
  });

  it("totals nested repeats", () => {
    const steps: WorkoutStep[] = [
      {
        kind: "repeat",
        repetitions: 3,
        steps: [
          {
            kind: "steady",
            durationSeconds: 30,
            target: { unit: "percentFtp", value: 120 },
          },
          { kind: "freeRide", durationSeconds: 30 },
        ],
      },
    ];
    expect(workoutDuration(steps)).toBe(180);
  });

  it("compiles runner-aligned intervals with expanded repeats", () => {
    const steps: WorkoutStep[] = [
      {
        kind: "steady",
        durationSeconds: 60,
        target: { unit: "percentFtp", value: 75 },
      },
      {
        kind: "repeat",
        repetitions: 2,
        steps: [
          {
            kind: "ramp",
            durationSeconds: 30,
            start: { unit: "watts", value: 180 },
            end: { unit: "percentFtp", value: 125 },
          },
          { kind: "freeRide", durationSeconds: 15 },
        ],
      },
    ];

    expect(compileWorkoutIntervals(steps, 201)).toEqual([
      {
        kind: "steady",
        durationSeconds: 60,
        startWatts: 150,
        endWatts: 150,
        freeRide: false,
      },
      {
        kind: "ramp",
        durationSeconds: 30,
        startWatts: 180,
        endWatts: 251,
        freeRide: false,
      },
      {
        kind: "freeRide",
        durationSeconds: 15,
        startWatts: null,
        endWatts: null,
        freeRide: true,
      },
      {
        kind: "ramp",
        durationSeconds: 30,
        startWatts: 180,
        endWatts: 251,
        freeRide: false,
      },
      {
        kind: "freeRide",
        durationSeconds: 15,
        startWatts: null,
        endWatts: null,
        freeRide: true,
      },
    ]);
  });

  it("formats short and long durations", () => {
    expect(formatDuration(65)).toBe("1:05");
    expect(formatDuration(3661)).toBe("1:01:01");
  });

  it("maps non-repeating arrow keys to manual power changes", () => {
    expect(manualPowerDeltaForKey("ArrowUp", false)).toBe(5);
    expect(manualPowerDeltaForKey("ArrowDown", false)).toBe(-5);
    expect(manualPowerDeltaForKey("ArrowUp", true)).toBeNull();
    expect(manualPowerDeltaForKey("Enter", false)).toBeNull();
  });

  it("formats metric-backed distance and speed in either display unit", () => {
    expect(formatDistance(10_000, "km")).toEqual({ value: "10.0", unit: "km" });
    expect(formatDistance(1609.344, "mi")).toEqual({ value: "1.00", unit: "mi" });
    expect(formatSpeed(32.18688, "mi")).toEqual({ value: "20.0", unit: "mph" });
    expect(formatSpeed(32.18688, "km")).toEqual({ value: "32.2", unit: "km/h" });
  });

  it("routes Shift+arrows to the bias and plain arrows to the target", () => {
    expect(rideKeyAction("ArrowUp", false, false)).toEqual({ kind: "power", delta: 5 });
    expect(rideKeyAction("ArrowDown", true, false)).toEqual({ kind: "bias", delta: -1 });
    expect(rideKeyAction("ArrowUp", true, true)).toBeNull();
    expect(rideKeyAction("a", true, false)).toBeNull();
  });

  it("keeps the bias inside 50–150 %", () => {
    expect(clampBias(100.4)).toBe(100);
    expect(clampBias(10)).toBe(50);
    expect(clampBias(999)).toBe(150);
  });

  it("averages power over a trailing time window", () => {
    const history = [0, 1, 2, 3, 4, 5].map((second) => ({
      timestampMs: second * 1000,
      powerWatts: second * 100,
    }));
    expect(withSmoothedPower(history, "instant").map((s) => s.displayPowerWatts)).toEqual([
      0, 100, 200, 300, 400, 500,
    ]);
    // 3 s window: at t=5 the samples at t=3,4,5 are included (t=2 is 3 s old and drops out).
    expect(withSmoothedPower(history, "3s").map((s) => s.displayPowerWatts)).toEqual([
      0, 50, 100, 200, 300, 400,
    ]);
    // 5 s window: at t=5 the samples at t=1..5 are included.
    expect(withSmoothedPower(history, "5s")[5].displayPowerWatts).toBe(300);
    // 10 s window covers everything here.
    expect(withSmoothedPower(history, "10s")[5].displayPowerWatts).toBe(250);
    expect(withSmoothedPower([], "10s")).toEqual([]);
  });

  it("derives standard open-ended zones from FTP and max HR", () => {
    expect(derivedPowerZones(200).map((zone) => zone.upperBound)).toEqual([
      110, 150, 180, 210, 240, 300, null,
    ]);
    expect(derivedHeartRateZones(200).map((zone) => zone.upperBound)).toEqual([
      120, 140, 160, 180, null,
    ]);
  });

  it("classifies zone boundaries inclusively", () => {
    const zones = derivedHeartRateZones(200);
    expect(zoneIndex(120, zones)).toBe(0);
    expect(zoneIndex(121, zones)).toBe(1);
    expect(zoneIndex(220, zones)).toBe(4);
  });

  it("totals variable sample intervals but excludes pauses and missing HR", () => {
    const samples = [
      { timestampMs: 0, powerWatts: 100, heartRateBpm: 110 },
      { timestampMs: 800, powerWatts: 160, heartRateBpm: null },
      { timestampMs: 2000, powerWatts: 190, heartRateBpm: 150 },
      { timestampMs: 12_000, powerWatts: 220, heartRateBpm: 180 },
    ].map((sample) => ({
      ...sample,
      cadenceRpm: null,
      speedKph: null,
      targetPowerWatts: null,
    }));
    expect(timeInZones(samples, derivedPowerZones(200), "power")).toEqual([
      0.8, 0, 1.2, 0, 0, 0, 0,
    ]);
    expect(timeInZones(samples, derivedHeartRateZones(200), "heartRate")).toEqual([
      0.8, 0, 0, 0, 0,
    ]);
  });

  it("totals work in kilojoules and skips dropouts", () => {
    const samples = [
      { timestampMs: 0, powerWatts: 100 },
      { timestampMs: 1000, powerWatts: 200 },
      // 10 s gap: the rider stepped off, so it is not billed as 300 W.
      { timestampMs: 11_000, powerWatts: 300 },
      { timestampMs: 12_000, powerWatts: 250 },
    ].map((sample) => ({
      ...sample,
      cadenceRpm: null,
      speedKph: null,
      heartRateBpm: null,
      targetPowerWatts: null,
    }));
    // 100 W for 1 s + 300 W for 1 s; the trailing sample has no interval yet.
    expect(workKilojoules(samples)).toBeCloseTo(0.4, 10);
    expect(workKilojoules([])).toBe(0);
    expect(workKilojoules(samples.slice(0, 1))).toBe(0);
  });

  it("averages power over riding time, not sample count", () => {
    const samples = [
      { timestampMs: 0, powerWatts: 100 },
      // One long block at 100 W outweighs the short 300 W samples that follow.
      { timestampMs: 4000, powerWatts: 300 },
      { timestampMs: 5000, powerWatts: 300 },
      // 10 s gap: neither the work nor the time counts.
      { timestampMs: 15_000, powerWatts: 500 },
    ].map((sample) => ({
      ...sample,
      cadenceRpm: null,
      speedKph: null,
      heartRateBpm: null,
      targetPowerWatts: null,
    }));
    expect(averagePowerWatts(samples)).toBeCloseTo(140, 10); // 0.7 kJ over 5 s
    expect(averagePowerWatts([])).toBeNull();
    expect(averagePowerWatts(samples.slice(0, 1))).toBeNull();
  });

  it("converts work to calories at cycling's gross efficiency", () => {
    expect(kilojoulesToKilocalories(0)).toBe(0);
    // Close to 1:1, which is why bike computers report the two side by side.
    expect(kilojoulesToKilocalories(1000)).toBeCloseTo(996, 0);
  });

  it("downsamples while retaining the session endpoints", () => {
    const samples = Array.from({ length: 100 }, (_, timestampMs) => ({
      timestampMs,
      powerWatts: timestampMs,
      cadenceRpm: null,
      speedKph: null,
      heartRateBpm: null,
      targetPowerWatts: null,
    }));
    const result = downsampleTelemetry(samples, 10);
    expect(result).toHaveLength(10);
    expect(result[0]).toBe(samples[0]);
    expect(result[9]).toBe(samples[99]);
  });

  it("compresses pause gaps and aligns chart time to elapsed ride time", () => {
    const samples = [0, 1000, 2000, 62_000, 63_000].map((timestampMs) => ({
      timestampMs,
      powerWatts: 100,
      cadenceRpm: null,
      speedKph: null,
      heartRateBpm: null,
      targetPowerWatts: null,
    }));
    expect(withActiveElapsed(samples).map((sample) => sample.activeElapsedMs)).toEqual([
      0, 1000, 2000, 2000, 3000,
    ]);
    const aligned = withActiveElapsed(samples, 4);
    expect(aligned[aligned.length - 1].activeElapsedMs).toBe(4000);
  });
});

describe("zone provenance and sync reporting", () => {
  const imported = {
    ...defaultTrainingZoneSettings,
    powerMode: "intervals" as const,
    powerZones: derivedPowerZones(300),
    heartRateMode: "intervals" as const,
    heartRateZones: derivedHeartRateZones(200),
  };

  it("treats imported zones like custom ones when picking the effective set", () => {
    expect(effectivePowerZones(imported, 200)).toEqual(derivedPowerZones(300));
    expect(effectiveHeartRateZones(imported, 180)).toEqual(derivedHeartRateZones(200));
    expect(effectivePowerZones(defaultTrainingZoneSettings, 200)).toEqual(derivedPowerZones(200));
    expect(zoneModeLabel.intervals).toBe("From Intervals.icu");
  });

  it("describes every item of a training-settings sync", () => {
    const base: TrainingSyncResult = {
      profile: {
        id: "p",
        name: "Blake",
        ftpWatts: 265,
        maxPowerWatts: 1000,
        maxHeartRateBpm: 190,
        riderWeightKg: 75,
        bikeWeightKg: 9,
        weightUnit: "kg",
        distanceUnit: "km",
      },
      zones: imported,
      ftp: { watts: 265, previousWatts: 250, source: "indoorFtp" },
      maxHeartRate: { bpm: 190, previousBpm: 190 },
      powerZones: { status: "imported" },
      heartRateZones: { status: "unchanged" },
    };
    expect(describeTrainingSync(base)).toBe(
      "FTP 265 W from your Intervals.icu indoor FTP (was 250 W) · Max HR 190 bpm (unchanged) · Power zones imported · Heart-rate zones already up to date",
    );
    expect(
      describeTrainingSync({
        ...base,
        ftp: { watts: 250, previousWatts: 250, source: "ftp" },
        maxHeartRate: null,
        powerZones: { status: "invalid", reason: "Intervals.icu returned power zones that do not increase" },
        heartRateZones: { status: "notConfigured" },
      }),
    ).toBe(
      "FTP 250 W from your Intervals.icu FTP (unchanged) · Intervals.icu has no max HR; yours is unchanged · Power zones not imported: Intervals.icu returned power zones that do not increase · Intervals.icu has no heart-rate zones",
    );
    expect(describeTrainingSync({ ...base, powerZones: { status: "syncOff" } })).toContain(
      "Power zones not imported (import is off)",
    );
  });
});
