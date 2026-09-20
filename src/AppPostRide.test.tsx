import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RunnerState } from "./types";
import { defaultRideDisplayPreferences } from "./rideScreens";
import { defaultTrainingZoneSettings } from "./types";

const listeners = vi.hoisted(() => ({
  runner: null as null | ((state: RunnerState) => void),
}));

const running: RunnerState = {
  status: "running",
  sessionId: "session",
  workoutName: "Threshold",
  elapsedSeconds: 60,
  totalSeconds: 60,
  intervalIndex: 0,
  intervalElapsedSeconds: 60,
  distanceMeters: 0,
  targetPowerWatts: 200,
  plannedTargetWatts: 200,
  manualErg: false,
  overrideActive: false,
  biasPercent: 100,
};

const session = {
  summary: {
    id: "session",
    workoutId: "workout",
    workoutName: "Threshold",
    startedAt: "2026-09-19T12:00:00Z",
    endedAt: "2026-09-19T12:30:00Z",
    elapsedSeconds: 1800,
    averagePowerWatts: 205,
    maxPowerWatts: 410,
    averageCadenceRpm: 88,
    estimatedDistanceMeters: 15000,
    distanceSource: "trainer" as const,
    distanceWeightKg: 84,
    completed: true,
  },
  samples: [],
};

vi.mock("./api", () => ({
  api: {
    profile: vi.fn(async () => ({ id: "rider", name: "Rider", ftpWatts: 200, maxPowerWatts: 1000, maxHeartRateBpm: 190, riderWeightKg: 75, bikeWeightKg: 9, weightUnit: "kg", distanceUnit: "km" })),
    workouts: vi.fn(async () => []),
    sessions: vi.fn(async () => []),
    devicesSnapshot: vi.fn(async () => ({ slots: [], scanning: false, scanError: null, sourcePreferences: { power: { mode: "auto" }, cadence: { mode: "auto" }, heartRate: { mode: "auto" } }, sources: { power: null, cadence: null, heartRate: null } })),
    runnerState: vi.fn(async () => running),
    powerSmoothing: vi.fn(async () => "instant"),
    trainingZones: vi.fn(async () => defaultTrainingZoneSettings),
    rideDisplayPreferences: vi.fn(async () => defaultRideDisplayPreferences),
    devMode: vi.fn(async () => false),
    session: vi.fn(async () => session),
    reportEvent: vi.fn(async () => undefined),
    reportError: vi.fn(async () => undefined),
    onTelemetry: vi.fn(async () => () => undefined),
    onRunnerState: vi.fn(async (handler: (state: RunnerState) => void) => { listeners.runner = handler; return () => undefined; }),
    onDeviceSlot: vi.fn(async () => () => undefined),
    onDeviceLog: vi.fn(async () => () => undefined),
  },
}));

import App from "./App";

afterEach(() => {
  cleanup();
  listeners.runner = null;
});

describe("post-ride navigation", () => {
  it("opens the saved ride detail automatically", async () => {
    render(<App />);
    await waitFor(() => expect(listeners.runner).not.toBeNull());

    await act(async () => {
      listeners.runner?.({ status: "finished", sessionId: "session", completed: true });
    });

    expect(await screen.findByRole("dialog", { name: "Threshold" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "View ride metrics" })).not.toBeInTheDocument();
  });
});
