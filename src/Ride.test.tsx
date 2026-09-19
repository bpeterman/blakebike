import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Ride } from "./App";
import { defaultSourcePreferences } from "./sourcePreferences";
import {
  defaultRideDisplayPreferences,
  defaultTrainingZoneSettings,
  type Profile,
  type RunnerState,
  type Telemetry,
  type Workout,
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

const structuredWorkout: Workout = {
  id: "workout",
  name: "Repeat Builder",
  description: "Expanded repeat test",
  source: "local",
  version: 1,
  createdAt: "2026-01-01T00:00:00Z",
  updatedAt: "2026-01-01T00:00:00Z",
  steps: [
    {
      kind: "steady",
      durationSeconds: 60,
      target: { unit: "percentFtp", value: 50 },
    },
    {
      kind: "repeat",
      repetitions: 2,
      steps: [
        {
          kind: "steady",
          durationSeconds: 30,
          target: { unit: "percentFtp", value: 100 },
        },
        { kind: "freeRide", durationSeconds: 15 },
      ],
    },
  ],
};

afterEach(cleanup);

describe("Ride charts", () => {
  it("always shows time in zone and collapses each session chart", () => {
    const { container } = render(
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
        displayPreferences={defaultRideDisplayPreferences}
        perform={async () => undefined}
      />,
    );
    expect(screen.queryByRole("checkbox", { name: "Time in zone" })).not.toBeInTheDocument();
    expect(screen.getByText("Power zones")).toBeInTheDocument();
    expect(screen.getByText("Heart-rate zones")).toBeInTheDocument();

    const [powerCollapse, heartRateCollapse] = screen.getAllByRole("button", { name: "Collapse" });
    expect(container.querySelector("#ride-power-chart")).toBeInTheDocument();
    expect(container.querySelector("#ride-heart-rate-chart")).toBeInTheDocument();

    fireEvent.click(powerCollapse);
    expect(powerCollapse).toHaveAttribute("aria-expanded", "false");
    expect(container.querySelector("#ride-power-chart")).not.toBeInTheDocument();
    expect(container.querySelector("#ride-heart-rate-chart")).toBeInTheDocument();

    fireEvent.click(heartRateCollapse);
    expect(container.querySelector("#ride-heart-rate-chart")).not.toBeInTheDocument();
  });

  it("shows expanded workout progress and both countdowns while running or paused", () => {
    const structuredRunner: RunnerState = {
      status: "running",
      sessionId: "session",
      workoutName: structuredWorkout.name,
      elapsedSeconds: 70,
      totalSeconds: 150,
      intervalIndex: 1,
      intervalElapsedSeconds: 10,
      targetPowerWatts: 200,
      plannedTargetWatts: 200,
      manualErg: false,
      overrideActive: false,
      biasPercent: 100,
    };
    const commonProps = {
      workouts: [structuredWorkout],
      selectedWorkout: structuredWorkout.id,
      setSelectedWorkout: vi.fn(),
      connected: true,
      onConnect: vi.fn(),
      telemetry,
      telemetryHistory: [telemetry],
      powerSmoothing: "instant" as const,
      onPowerSmoothing: vi.fn(),
      sourcePreferences: defaultSourcePreferences,
      onSourcePreference: vi.fn(),
      profile,
      trainingZones: defaultTrainingZoneSettings,
      displayPreferences: defaultRideDisplayPreferences,
      perform: async () => undefined,
    };
    const { rerender } = render(<Ride {...commonProps} runner={structuredRunner} />);

    expect(screen.queryByRole("heading", { name: structuredWorkout.name })).not.toBeInTheDocument();
    expect(screen.getByText(structuredWorkout.name)).toHaveClass("label");
    expect(screen.getByText("Block 2 of 5")).toBeInTheDocument();
    expect(screen.getByLabelText("Block 1 of 5, completed")).toHaveAttribute("data-state", "completed");
    expect(screen.getByLabelText("Block 2 of 5, current")).toHaveAttribute("data-state", "current");
    expect(screen.getByLabelText("Block 3 of 5, upcoming")).toHaveAttribute("data-state", "upcoming");
    expect(screen.getByText("0:20")).toBeInTheDocument();
    expect(screen.getByText("1:20")).toBeInTheDocument();
    const timeline = screen.getByRole("button", { name: "Skip block" }).closest("[data-ride-card]");
    const controls = screen.getByText("TARGET & BIAS").closest("[data-ride-card]");
    const powerChart = screen.getAllByText("FULL SESSION")[0].closest("[data-ride-card]");
    expect(timeline?.nextElementSibling).toBe(controls);
    expect(controls?.nextElementSibling).toBe(powerChart);
    expect(screen.queryByRole("button", { name: "Pause" })).not.toBeInTheDocument();
    expect(screen.getByLabelText("Workout bias")).toBeInTheDocument();

    const pausedRunner: RunnerState = {
      ...structuredRunner,
      status: "paused",
      intervalElapsedSeconds: 10,
    };
    rerender(<Ride {...commonProps} runner={pausedRunner} />);
    expect(screen.getByText("200 W · Paused")).toBeInTheDocument();
    expect(screen.getByText("0:20")).toBeInTheDocument();

    rerender(<Ride {...commonProps} runner={{ ...structuredRunner, overrideActive: true, targetPowerWatts: 225 }} />);
    expect(screen.getByRole("button", { name: "Reset override · 200 W" })).toBeInTheDocument();
  });

  it("hides cards and renders visible cards in the saved order", () => {
    const displayPreferences = {
      version: 2 as const,
      cards: [
        { id: "heartRate" as const, visible: true },
        { id: "power" as const, visible: false },
        { id: "speed" as const, visible: true },
        ...defaultRideDisplayPreferences.cards.filter(
          ({ id }) => !["heartRate", "power", "speed"].includes(id),
        ).map((card) => ({ ...card, visible: false })),
      ],
    };
    const { container } = render(
      <Ride
        workouts={[]}
        selectedWorkout={null}
        setSelectedWorkout={vi.fn()}
        connected
        onConnect={vi.fn()}
        runner={runner}
        telemetry={telemetry}
        telemetryHistory={[telemetry]}
        powerSmoothing="instant"
        onPowerSmoothing={vi.fn()}
        sourcePreferences={defaultSourcePreferences}
        onSourcePreference={vi.fn()}
        profile={profile}
        trainingZones={defaultTrainingZoneSettings}
        displayPreferences={displayPreferences}
        perform={async () => undefined}
      />,
    );

    expect(screen.queryByText("POWER")).not.toBeInTheDocument();
    expect(screen.getByText("HEART RATE")).toBeInTheDocument();
    expect(screen.getByText("SPEED")).toBeInTheDocument();
    expect(
      [...container.querySelectorAll("[data-ride-card]")].map((card) =>
        card.getAttribute("data-ride-card"),
      ),
    ).toEqual(["heartRate", "speed"]);
  });
  it("explains an error that ended before a ride could be saved", () => {
    const errored: RunnerState = {
      status: "error",
      message: "Trainer control command timed out",
      sessionId: null,
    };
    render(
      <Ride
        workouts={[]}
        selectedWorkout={null}
        setSelectedWorkout={vi.fn()}
        connected
        onConnect={vi.fn()}
        runner={errored}
        telemetry={telemetry}
        telemetryHistory={[]}
        powerSmoothing="instant"
        onPowerSmoothing={vi.fn()}
        sourcePreferences={defaultSourcePreferences}
        onSourcePreference={vi.fn()}
        profile={profile}
        trainingZones={defaultTrainingZoneSettings}
        displayPreferences={defaultRideDisplayPreferences}
        perform={async () => undefined}
      />,
    );
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("Ride ended: Trainer control command timed out.");
    expect(screen.getByText("Start a ride")).toBeInTheDocument();
  });

  it("tells the rider when the trainer link is lost or degraded, and nothing otherwise", () => {
    const rideWith = (state: RunnerState) => (
      <Ride
        workouts={[]}
        selectedWorkout={null}
        setSelectedWorkout={vi.fn()}
        connected
        onConnect={vi.fn()}
        runner={state}
        telemetry={telemetry}
        telemetryHistory={[telemetry]}
        powerSmoothing="instant"
        onPowerSmoothing={vi.fn()}
        sourcePreferences={defaultSourcePreferences}
        onSourcePreference={vi.fn()}
        profile={profile}
        trainingZones={defaultTrainingZoneSettings}
        displayPreferences={defaultRideDisplayPreferences}
        perform={async () => undefined}
      />
    );
    const { rerender } = render(rideWith({ ...runner, control: "lost" }));
    expect(screen.getByRole("status")).toHaveTextContent("Trainer link lost");
    rerender(rideWith({ ...runner, control: "degraded" }));
    expect(screen.getByRole("status")).toHaveTextContent("Trainer not acknowledging targets");
    rerender(rideWith({ ...runner, control: "ok" }));
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    rerender(rideWith(runner));
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("shows recording warnings during and after a ride, and clears recovered warnings", () => {
    const rideWith = (state: RunnerState) => (
      <Ride workouts={[]} selectedWorkout={null} setSelectedWorkout={vi.fn()} connected
        onConnect={vi.fn()} runner={state} telemetry={telemetry} telemetryHistory={[telemetry]}
        powerSmoothing="instant" onPowerSmoothing={vi.fn()} sourcePreferences={defaultSourcePreferences}
        onSourcePreference={vi.fn()} profile={profile} trainingZones={defaultTrainingZoneSettings}
        displayPreferences={defaultRideDisplayPreferences} perform={async () => undefined} />
    );
    const message = "Ride data is not being saved. Retrying; check available disk space and keep the app open.";
    const { rerender } = render(rideWith({ ...runner, recordingWarning: message }));
    expect(screen.getByRole("alert")).toHaveTextContent(message);
    rerender(rideWith({ ...runner, recordingWarning: null }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    rerender(rideWith({ status: "finished", sessionId: "session", completed: true, saveWarning: "No ride measurements were saved." }));
    expect(screen.getByRole("alert")).toHaveTextContent("No ride measurements were saved.");
    expect(screen.queryByText("Ride saved")).not.toBeInTheDocument();
  });

  it("makes unconfirmed pause visible until the trainer acknowledges it", () => {
    const paused: RunnerState = { ...runner, status: "paused", control: "degraded" };
    const props = { workouts: [], selectedWorkout: null, setSelectedWorkout: vi.fn(), connected: true,
      onConnect: vi.fn(), telemetry, telemetryHistory: [telemetry], powerSmoothing: "instant" as const,
      onPowerSmoothing: vi.fn(), sourcePreferences: defaultSourcePreferences, onSourcePreference: vi.fn(),
      profile, trainingZones: defaultTrainingZoneSettings, displayPreferences: defaultRideDisplayPreferences,
      perform: async () => undefined };
    const { rerender } = render(<Ride {...props} runner={paused} />);
    expect(screen.getByRole("status")).toHaveTextContent("Trainer pause not confirmed");
    expect(screen.getByRole("status")).toHaveTextContent("trainer may still be applying resistance");
    rerender(<Ride {...props} runner={{ ...paused, control: "lost" }} />);
    expect(screen.getByRole("status")).toHaveTextContent("ride clock is paused");
    rerender(<Ride {...props} runner={{ ...paused, control: "ok" }} />);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

});
