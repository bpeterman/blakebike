import { describe, expect, it } from "vitest";
import { compileWorkoutIntervals, type WorkoutStep } from "./types";
import {
  profileScale,
  profileSegments,
  profileShapes,
  profileStats,
  profileSummary,
  repeatSpans,
  shapesWithin,
  timeAxisTicks,
} from "./workoutProfileModel";

const FTP = 200;

const steady = (durationSeconds: number, value: number): WorkoutStep => ({
  kind: "steady",
  durationSeconds,
  target: { unit: "percentFtp", value },
});

const ramp = (durationSeconds: number, start: number, end: number): WorkoutStep => ({
  kind: "ramp",
  durationSeconds,
  start: { unit: "percentFtp", value: start },
  end: { unit: "percentFtp", value: end },
});

const free = (durationSeconds: number): WorkoutStep => ({ kind: "freeRide", durationSeconds });

const mixed: WorkoutStep[] = [
  ramp(600, 50, 75),
  { kind: "repeat", repetitions: 3, steps: [steady(300, 100), steady(300, 50)] },
  free(600),
];

describe("profileSegments", () => {
  it("carries exactly the runner's intervals alongside each expanded step", () => {
    const segments = profileSegments(mixed, FTP);
    expect(segments.map((segment) => segment.interval)).toEqual(
      compileWorkoutIntervals(mixed, FTP),
    );
    expect(segments.map((segment) => [...segment.path])).toEqual([
      [0], [1, 0], [1, 1], [1, 0], [1, 1], [1, 0], [1, 1], [2],
    ]);
  });
});

describe("profileStats", () => {
  it("weights averages over targeted time and reports free ride separately", () => {
    const stats = profileStats(profileSegments(mixed, FTP), FTP);
    expect(stats.totalSeconds).toBe(3000);
    expect(stats.freeRideSeconds).toBe(600);
    expect(stats.blockCount).toBe(8);
    // Ramp 50→75 averages 62.5 % for 600 s; 3 × (100 % + 50 %) for 300 s each.
    const expectedAverage = (62.5 * 600 + 3 * (100 * 300 + 50 * 300)) / 2400;
    expect(stats.averagePercentFtp).toBeCloseTo(expectedAverage, 5);
    expect(stats.peakWatts).toBe(200);
    expect(stats.peakPercentFtp).toBe(100);
  });

  it("estimates stress from the squared intensity per block, integrating ramps", () => {
    const hour = profileStats(profileSegments([steady(3600, 100)], FTP), FTP);
    expect(hour.estimatedStress).toBeCloseTo(100, 5);
    const halfHour = profileStats(profileSegments([steady(1800, 80)], FTP), FTP);
    expect(halfHour.estimatedStress).toBeCloseTo(0.64 * 0.5 * 100, 5);
    // A ramp from 0 to FTP has mean square 1/3, not (1/2)² = 1/4.
    const rampUp = profileStats(profileSegments([ramp(3600, 0, 100)], FTP), FTP);
    expect(rampUp.estimatedStress).toBeCloseTo(100 / 3, 5);
  });

  it("returns null intensity figures when nothing is targeted", () => {
    const stats = profileStats(profileSegments([free(600)], FTP), FTP);
    expect(stats).toMatchObject({
      totalSeconds: 600,
      freeRideSeconds: 600,
      averagePercentFtp: null,
      estimatedStress: null,
      peakWatts: 0,
      peakPercentFtp: null,
    });
    expect(profileStats([], FTP).blockCount).toBe(0);
  });
});

describe("profileScale", () => {
  it("keeps headroom above FTP so easy workouts do not fill the frame", () => {
    const segments = profileSegments([steady(600, 60)], FTP);
    const scale = profileScale(segments, { width: 500, height: 100, ftpWatts: FTP });
    expect(scale.maxWatts).toBe(240);
    expect(scale.y(240)).toBe(0);
    expect(scale.y(0)).toBe(100);
    expect(scale.y(120)).toBe(50);
    expect(scale.x(300)).toBe(250);
  });

  it("grows the axis to the peak target and clamps out-of-range values", () => {
    const segments = profileSegments([steady(60, 150)], FTP);
    const scale = profileScale(segments, { width: 100, height: 100, ftpWatts: FTP });
    expect(scale.maxWatts).toBe(300);
    expect(scale.y(400)).toBe(0);
    expect(scale.y(-5)).toBe(100);
  });

  it("is safe for an empty workout", () => {
    const scale = profileScale([], { width: 100, height: 50, ftpWatts: FTP });
    expect(scale.totalSeconds).toBe(0);
    expect(scale.x(0)).toBe(0);
    expect(scale.maxWatts).toBe(240);
  });
});

describe("profileShapes", () => {
  it("draws steady blocks as rectangles and ramps as trapezoids", () => {
    const steps = [steady(300, 100), ramp(300, 50, 100)];
    const segments = profileSegments(steps, FTP);
    const scale = profileScale(segments, { width: 240, height: 120, ftpWatts: FTP });
    const [flat, sloped] = profileShapes(segments, scale, 1);
    const ftpY = scale.y(200);
    const halfY = scale.y(100);
    expect(flat.points).toEqual([[0, ftpY], [119, ftpY], [119, 120], [0, 120]]);
    expect(sloped.points).toEqual([[120, halfY], [240, ftpY], [240, 120], [120, 120]]);
    expect(flat.width).toBe(119);
    expect(sloped.width).toBe(120);
  });

  it("keeps a 40 × 30/30 set proportional by dropping gaps on narrow blocks", () => {
    const steps: WorkoutStep[] = [
      { kind: "repeat", repetitions: 40, steps: [steady(30, 120), steady(30, 50)] },
    ];
    const segments = profileSegments(steps, FTP);
    const scale = profileScale(segments, { width: 160, height: 100, ftpWatts: FTP });
    const shapes = profileShapes(segments, scale, 1);
    expect(shapes).toHaveLength(80);
    const totalWidth = shapes.reduce((sum, shape) => sum + shape.width, 0);
    expect(totalWidth).toBeCloseTo(160, 6);
    shapes.forEach((shape) => expect(shape.width).toBeCloseTo(2, 6));
  });

  it("gives free ride a nominal placeholder height", () => {
    const segments = profileSegments([free(60)], FTP);
    const scale = profileScale(segments, { width: 100, height: 100, ftpWatts: FTP });
    const [shape] = profileShapes(segments, scale);
    expect(shape.kind).toBe("freeRide");
    expect(shape.startWatts).toBeNull();
    expect(shape.points[0][1]).toBe(scale.y(scale.freeRideWatts));
  });
});

describe("timeAxisTicks", () => {
  it("chooses spacing from the available pixel width", () => {
    expect(timeAxisTicks(3600, 800)).toEqual([300, 600, 900, 1200, 1500, 1800, 2100, 2400, 2700, 3000, 3300]);
    expect(timeAxisTicks(3600, 200)).toEqual([1800]);
    expect(timeAxisTicks(3600, 240)).toEqual([900, 1800, 2700]);
    expect(timeAxisTicks(600, 800)).toEqual([60, 120, 180, 240, 300, 360, 420, 480, 540]);
    expect(timeAxisTicks(4 * 3600, 100)).toEqual([3600, 7200, 10800]);
  });

  it("excludes the endpoints and handles degenerate input", () => {
    expect(timeAxisTicks(300, 800)).toEqual([60, 120, 180, 240]);
    expect(timeAxisTicks(0, 800)).toEqual([]);
    expect(timeAxisTicks(600, 0)).toEqual([]);
  });
});

describe("repeatSpans", () => {
  it("covers every iteration of each group and reports nesting depth", () => {
    const steps: WorkoutStep[] = [
      steady(60, 50),
      {
        kind: "repeat",
        repetitions: 2,
        steps: [steady(30, 100), { kind: "repeat", repetitions: 2, steps: [free(5)] }],
      },
    ];
    const spans = repeatSpans(steps, profileSegments(steps, FTP));
    expect(spans.map((span) => ({ ...span, path: [...span.path] }))).toEqual([
      { path: [1], depth: 0, repetitions: 2, startSeconds: 60, endSeconds: 140 },
      { path: [1, 1], depth: 1, repetitions: 2, startSeconds: 90, endSeconds: 140 },
    ]);
  });

  it("is empty for flat workouts", () => {
    expect(repeatSpans(mixed.slice(0, 1), profileSegments(mixed.slice(0, 1), FTP))).toEqual([]);
  });
});

describe("shapesWithin", () => {
  it("selects every block produced by a repeat group", () => {
    const segments = profileSegments(mixed, FTP);
    const scale = profileScale(segments, { width: 300, height: 100, ftpWatts: FTP });
    const shapes = profileShapes(segments, scale);
    expect(shapesWithin(shapes, [1])).toHaveLength(6);
    expect(shapesWithin(shapes, [1, 0])).toHaveLength(3);
    expect(shapesWithin(shapes, [2])).toHaveLength(1);
    expect(shapesWithin(shapes, [7])).toHaveLength(0);
  });
});

describe("profileSummary", () => {
  it("describes the workout in plain language", () => {
    expect(profileSummary(profileStats(profileSegments(mixed, FTP), FTP))).toBe(
      "50:00 workout, 8 blocks, peak 100% FTP, 10:00 free ride",
    );
    expect(profileSummary(profileStats([], FTP))).toBe("Empty workout");
    expect(profileSummary(profileStats(profileSegments([steady(60, 90)], FTP), FTP))).toBe(
      "1:00 workout, 1 block, peak 90% FTP",
    );
  });
});
