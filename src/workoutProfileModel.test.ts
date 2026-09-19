import { describe, expect, it } from "vitest";
import { compileWorkoutIntervals, type WorkoutStep } from "./types";
import {
  profileLabel,
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

  it("applies the ride bias to every target, leaving durations and free ride alone", () => {
    const biased = profileSegments(mixed, FTP, 110);
    expect(biased.map((segment) => segment.interval)).toEqual(
      compileWorkoutIntervals(mixed, FTP, 110),
    );
    expect(biased[0].interval).toMatchObject({ startWatts: 110, endWatts: 165, durationSeconds: 600 });
    expect(biased[1].interval.startWatts).toBe(220);
    expect(biased[7].interval).toMatchObject({ startWatts: null, endWatts: null, freeRide: true });
    expect(profileStats(biased, FTP).peakWatts).toBe(220);
  });
});

describe("profileLabel", () => {
  const FONT = 10;
  const labelsFor = (steps: WorkoutStep[], width: number, height: number, bias = 100) => {
    const segments = profileSegments(steps, FTP, bias);
    const scale = profileScale(segments, { width, height, ftpWatts: FTP });
    return profileShapes(segments, scale).map((shape) => profileLabel(shape, scale, FONT));
  };

  it("writes the target and duration inside a block that can hold both lines", () => {
    const [label] = labelsFor([steady(300, 100)], 200, 100);
    expect(label).toMatchObject({ placement: "inside", target: "200 W", duration: "5:00", x: 100 });
    // Two lines of 12.5 px sit 4 px above the baseline.
    expect(label?.y).toBe(100 - 4 - 12.5);
    expect(label?.lineHeight).toBe(12.5);
  });

  it("floats the label above a block too short to hold it, and describes ramps and free ride", () => {
    // 20 % of FTP against a 240 W axis is 8.3 px tall on a 100 px plot.
    const [short, sloped, free] = labelsFor([steady(300, 20), ramp(300, 50, 100), { kind: "freeRide", durationSeconds: 300 }], 600, 100);
    expect(short).toMatchObject({ placement: "above", target: "40 W" });
    expect(short?.y).toBeCloseTo(100 - 100 / 6 - 4 - 12.5, 5);
    expect(sloped).toMatchObject({ placement: "inside", target: "100–200 W" });
    expect(free).toMatchObject({ placement: "inside", target: "Free ride", duration: "5:00" });
  });

  it("stands a single line on end in a block too narrow for horizontal text", () => {
    // 30 px and 15 px wide; 220 W is 91.7 px tall and 110 W is 45.8 px tall on a 100 px plot.
    const [over, under] = labelsFor([steady(120, 110), steady(60, 55)], 45, 100);
    expect(over).toEqual({
      x: 14.5, y: 96, lineHeight: 12.5, placement: "vertical", target: "220 W", duration: "2:00",
    });
    // "110 W · 1:00" needs 74 px but the block only has 38 px of room: drop the duration.
    expect(under).toMatchObject({ placement: "vertical", target: "110 W", duration: null });
    expect(under?.x).toBeCloseTo(37.5, 5);
  });

  it("is omitted for blocks narrower than a glyph or too short for even the target", () => {
    // 20 px and 10 px wide: a glyph needs 12 px.
    const [over, under] = labelsFor([steady(120, 110), steady(60, 55)], 30, 100);
    expect(over?.placement).toBe("vertical");
    expect(under).toBeNull();
    // "Free ride" is too wide for 50 px and, at 42 px of room, too tall to stand on end.
    const [steady50, free50] = labelsFor([steady(300, 100), { kind: "freeRide", durationSeconds: 300 }], 100, 100);
    expect(steady50?.placement).toBe("inside");
    expect(free50).toBeNull();
    // A 20 px plot has no room inside, above, or on end.
    expect(labelsFor([steady(300, 100)], 200, 20)).toEqual([null]);
  });

  it("follows the bias", () => {
    expect(labelsFor([steady(300, 100)], 200, 100, 110)[0]?.target).toBe("220 W");
    expect(labelsFor([steady(300, 100)], 200, 100, 90)[0]?.target).toBe("180 W");
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
    // A ramp from 0 to FTP has mean square 1/3, not (1/2)² = 1/4. The runner
    // floors targets at 1 W, so the ramp actually starts at 1 W.
    const rampUp = profileStats(profileSegments([ramp(3600, 0, 100)], FTP), FTP);
    expect(rampUp.estimatedStress).toBeCloseTo(((1 + FTP + FTP * FTP) / 3 / (FTP * FTP)) * 100, 5);
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
