import type { Bin, ReadyComparison } from "./dualPower";

/**
 * A plausible finished comparison for tests and the browser harness: a
 * trainer reading about 5 W high with a small warm-up drift, over a 25 minute
 * ride. Not a substitute for the Rust known-answer tests; this only exercises
 * presentation.
 */
export function readyComparisonFixture(overrides: Partial<ReadyComparison> = {}): ReadyComparison {
  const bin = (lower: number, upper: number, count: number, meanPercent: number): Bin => ({
    lower,
    upper,
    count,
    meanWatts: (meanPercent / 100) * ((lower + upper) / 2),
    meanPercent,
    sdWatts: 4.2,
  });
  const traces = Array.from({ length: 300 }, (_, index) => {
    const elapsedSeconds = index * 5;
    const base = 200 + 60 * Math.sin(index / 12) + (index % 40 < 3 ? -190 : 0);
    const gap = index >= 120 && index < 126;
    return {
      elapsedSeconds,
      a: gap ? null : Math.max(0, base * 1.02 + 5),
      b: gap ? null : Math.max(0, base),
    };
  });
  return {
    status: "ready",
    sessionId: "session",
    workoutName: "Threshold",
    startedAt: "2026-09-19T12:00:00Z",
    elapsedSeconds: 1500,
    a: {
      role: "trainer",
      device: {
        role: "trainer",
        id: "kickr",
        name: "KICKR CORE",
        transport: "ble",
        simulated: false,
        manufacturer: "Wahoo",
        model: "KICKR CORE",
        firmware: "1.2.3",
        lastCalibration: null,
      },
      readings: 5_820,
      rateHz: 3.9,
      seconds: 1_480,
      meanWatts: 214.2,
      maxWatts: 412,
    },
    b: {
      role: "power",
      device: {
        role: "power",
        id: "assioma",
        name: "Assioma DUO",
        transport: "ble",
        simulated: false,
        manufacturer: "Favero",
        model: null,
        firmware: "5.14",
        lastCalibration: {
          at: "2026-09-19T10:30:00Z",
          kind: "zeroOffset",
          offsetRaw: 1019,
          previousOffsetRaw: 1023,
        },
      },
      readings: 1_478,
      rateHz: 1.0,
      seconds: 1_476,
      meanWatts: 209.1,
      maxWatts: 404,
    },
    signConvention: "Difference is trainer minus power meter: positive means the trainer reads higher.",
    coverage: {
      rideSeconds: 1500,
      overlapSeconds: 1480,
      overlapStartSeconds: 12,
      overlapEndSeconds: 1492,
      comparedSeconds: 1_312,
      excludedSeconds: 168,
      coastingSeconds: 96,
      transientSeconds: 72,
    },
    exclusion: { minWatts: 30, transientWattsPerSecond: 50, guardSeconds: 3 },
    lag: {
      seconds: 1.75,
      correlation: 0.93,
      confident: true,
      searchSeconds: 10,
      note: "The trainer trails the power meter by 1.75 s; the meter was shifted to match before comparing.",
    },
    windows: [
      { windowSeconds: 1, count: 1_312, meanWatts: 5.1, meanPercent: 2.4, sdWatts: 9.8, loaLowWatts: -14.1, loaHighWatts: 24.3, p95AbsWatts: 18, p95AbsPercent: 8.2, maxAbsWatts: 41, maxAbsPercent: 19.5 },
      { windowSeconds: 3, count: 1_312, meanWatts: 5.1, meanPercent: 2.4, sdWatts: 6.2, loaLowWatts: -7.1, loaHighWatts: 17.3, p95AbsWatts: 12, p95AbsPercent: 5.6, maxAbsWatts: 27, maxAbsPercent: 12.1 },
      { windowSeconds: 30, count: 1_312, meanWatts: 5.0, meanPercent: 2.4, sdWatts: 2.1, loaLowWatts: 0.9, loaHighWatts: 9.1, p95AbsWatts: 8, p95AbsPercent: 3.9, maxAbsWatts: 11, maxAbsPercent: 5.4 },
    ],
    headlineWindowSeconds: 3,
    trend: { interceptWatts: 2.1, slopePercent: 1.4 },
    warmup: { minutes: 10, firstMeanPercent: 3.6, restMeanPercent: 1.8, firstCount: 540, restCount: 772 },
    byMinute: Array.from({ length: 24 }, (_, minute) => bin(minute, minute + 1, 50, minute < 10 ? 3.6 - minute * 0.12 : 1.8 + Math.sin(minute) * 0.3)),
    minuteBinMinPairs: 20,
    byPower: [bin(100, 150, 61, 3.1), bin(150, 200, 402, 2.7), bin(200, 250, 588, 2.3), bin(250, 300, 190, 2.0), bin(300, 350, 71, 1.7)],
    powerBinWatts: 50,
    byCadence: { source: "power", bins: [bin(70, 80, 48, 3.0), bin(80, 90, 611, 2.5), bin(90, 100, 590, 2.2), bin(100, 110, 63, 1.9)] },
    cadenceBinRpm: 10,
    binMinPairs: 30,
    traces,
    blandAltman: Array.from({ length: 400 }, (_, index) => {
      const mean = 120 + ((index * 37) % 260);
      return [mean, 2.1 + mean * 0.014 + Math.sin(index) * 6] as [number, number];
    }),
    ...overrides,
  };
}
