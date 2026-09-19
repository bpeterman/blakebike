import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Ride } from "./App";
import { defaultSourcePreferences } from "./sourcePreferences";
import {
  defaultTrainingZoneSettings,
  type Profile,
  type RunnerState,
  type Telemetry,
} from "./types";

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

const runner: RunnerState = {
  status: "running",
  sessionId: "session",
  workoutName: "Free Ride",
  elapsedSeconds: 30,
  totalSeconds: null,
  intervalIndex: 0,
  intervalElapsedSeconds: 30,
  targetPowerWatts: 100,
  plannedTargetWatts: null,
  manualErg: true,
  overrideActive: false,
  biasPercent: 100,
};

const telemetry: Telemetry = {
  timestampMs: 1_000,
  powerWatts: 150,
  cadenceRpm: 85,
  speedKph: 30,
  heartRateBpm: 140,
  targetPowerWatts: 100,
};

afterEach(cleanup);

describe("Ride charts", () => {
  it("offers the persisted time-in-zone chart toggle", () => {
    const onRideDisplay = vi.fn();
    render(
      <Ride
        workouts={[]}
        selectedWorkout={null}
        setSelectedWorkout={vi.fn()}
        connected
        onConnect={vi.fn()}
        runner={runner}
        telemetry={telemetry}
        telemetryHistory={[telemetry, { ...telemetry, timestampMs: 2_000 }]}
        powerSmoothing="instant"
        onPowerSmoothing={vi.fn()}
        sourcePreferences={defaultSourcePreferences}
        onSourcePreference={vi.fn()}
        profile={profile}
        trainingZones={defaultTrainingZoneSettings}
        rideDisplay={{ showTimeInZone: false }}
        onRideDisplay={onRideDisplay}
        perform={async () => undefined}
      />,
    );
    fireEvent.click(screen.getByRole("checkbox", { name: "Time in zone" }));
    expect(onRideDisplay).toHaveBeenCalledWith({ showTimeInZone: true });
    expect(screen.getAllByText("Power").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Heart rate").length).toBeGreaterThan(0);
  });
});
