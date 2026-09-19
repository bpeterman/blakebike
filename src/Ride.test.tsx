import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Ride } from "./App";
import { emptySlot } from "./devices";
import { defaultSourcePreferences } from "./sourcePreferences";
import {
  deviceRoles,
  defaultRideDisplayPreferences,
  defaultTrainingZoneSettings,
  type DevicesSnapshot,
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

const hub: DevicesSnapshot = {
  scanning: false,
  scanError: null,
  antAdapter: { status: "notAttached" },
  sourcePreferences: defaultSourcePreferences,
  sources: {
    power: { role: "trainer", fallback: false },
    cadence: { role: "trainer", fallback: false },
    heartRate: { role: "heartRate", fallback: false },
  },
  slots: deviceRoles.map(emptySlot),
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
        hub={hub}
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
      hub,
      profile,
      trainingZones: defaultTrainingZoneSettings,
      displayPreferences: defaultRideDisplayPreferences,
      perform: async () => undefined,
    };
    const { container, rerender } = render(<Ride {...commonProps} runner={structuredRunner} />);

    expect(screen.queryByRole("heading", { name: structuredWorkout.name })).not.toBeInTheDocument();
    expect(screen.getByText(structuredWorkout.name)).toHaveClass("label");
    expect(screen.getByText("Block 2 of 5")).toBeInTheDocument();
    const timelineBlocks = [...container.querySelectorAll<SVGPolygonElement>(".workout-timeline polygon[data-state]")];
    expect(timelineBlocks.map((block) => block.dataset.state)).toEqual([
      "completed", "current", "upcoming", "upcoming", "upcoming",
    ]);
    expect(timelineBlocks[1]).toHaveClass("current");
    expect(container.querySelectorAll(".workout-timeline .workout-profile-progress")).toHaveLength(2);
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

  it("labels timeline blocks with biased targets and durations that follow the bias", () => {
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
      hub,
      profile,
      trainingZones: defaultTrainingZoneSettings,
      displayPreferences: defaultRideDisplayPreferences,
      perform: async () => undefined,
    };
    const { container, rerender } = render(<Ride {...commonProps} runner={structuredRunner} />);
    const labelText = () =>
      [...container.querySelectorAll<SVGTextElement>(".workout-timeline .workout-profile-block-label")].map((label) => [
        label.dataset.labelFor,
        label.querySelector(".workout-profile-block-target")?.textContent,
        label.querySelector(".workout-profile-block-duration")?.textContent,
      ]);
    // On the 600 px fallback canvas the 15 s free-ride blocks are 59 and 60 px wide, and "Free ride"
    // is estimated at 59.8 px, so only the last one (which gives up no gap) is labelled.
    expect(labelText()).toEqual([
      ["0", "100 W", "1:00"],
      ["1.0", "200 W", "0:30"],
      ["1.0", "200 W", "0:30"],
      ["1.1", "Free ride", "0:15"],
    ]);
    expect(container.querySelector(".timeline-target")).toHaveTextContent("200 W");

    rerender(<Ride {...commonProps} runner={{ ...structuredRunner, biasPercent: 110, targetPowerWatts: 220 }} />);
    expect(labelText()).toEqual([
      ["0", "110 W", "1:00"],
      ["1.0", "220 W", "0:30"],
      ["1.0", "220 W", "0:30"],
      ["1.1", "Free ride", "0:15"],
    ]);
    expect(container.querySelector(".timeline-target")).toHaveTextContent("220 W");
    expect(screen.getByText("Plan 200 W · 110% bias")).toBeInTheDocument();
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
        hub={hub}
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
        hub={hub}
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
        hub={hub}
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
        onSourcePreference={vi.fn()} hub={hub} profile={profile} trainingZones={defaultTrainingZoneSettings}
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
      onPowerSmoothing: vi.fn(), sourcePreferences: defaultSourcePreferences, onSourcePreference: vi.fn(), hub,
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

  it("keeps stats for nerds off by default and shows every device's data when it is on", () => {
    const now = Date.now();
    const nerdHub: DevicesSnapshot = {
      ...hub,
      sources: {
        power: { role: "power", fallback: false },
        cadence: { role: "power", fallback: false },
        heartRate: { role: "trainer", fallback: true },
      },
      slots: [
        {
          ...emptySlot("trainer"),
          state: {
            status: "controlling",
            device: { id: "k", name: "KICKR CORE", transport: "ble", simulated: false, rssi: -55, capabilities: ["ftms"] },
          },
          stats: {
            ...emptySlot("trainer").stats,
            samples: 1_200,
            parseFailures: 2,
            drops: 1,
            rateHz: 2,
            rssi: -55,
            manufacturer: "Wahoo",
            model: "KICKR CORE",
            firmware: "1.2.3",
            connectedSinceMs: now - 125_000,
            lastSampleMs: now - 400,
            lastRawHex: "44 02 c8 00",
            lastReading: "200 W · 88 rpm",
          },
        },
        {
          ...emptySlot("heartRate"),
          state: { status: "reconnecting", name: "HRM-Pro" },
          stats: { ...emptySlot("heartRate").stats, drops: 3, reconnectAttempt: 2 },
        },
        {
          ...emptySlot("power"),
          state: {
            status: "ready",
            device: { id: "p", name: "Assioma", transport: "ant", simulated: false, rssi: null, capabilities: ["cyclingPower"] },
          },
          stats: { ...emptySlot("power").stats, rateHz: 1, lastReading: "204 W · 90 rpm", lastSampleMs: now - 200 },
        },
        emptySlot("cadence"),
      ],
    };
    const displayPreferences = {
      version: 2 as const,
      cards: defaultRideDisplayPreferences.cards.map((card) => ({
        ...card,
        visible: card.id === "deviceStats",
      })),
    };
    const props = {
      workouts: [], selectedWorkout: null, setSelectedWorkout: vi.fn(), connected: true,
      onConnect: vi.fn(), runner, telemetry, telemetryHistory: [telemetry],
      powerSmoothing: "instant" as const, onPowerSmoothing: vi.fn(),
      sourcePreferences: defaultSourcePreferences, onSourcePreference: vi.fn(),
      profile, trainingZones: defaultTrainingZoneSettings, perform: async () => undefined,
    };

    const { container, rerender } = render(
      <Ride {...props} hub={nerdHub} displayPreferences={defaultRideDisplayPreferences} />,
    );
    expect(screen.queryByText("STATS FOR NERDS")).not.toBeInTheDocument();

    rerender(<Ride {...props} hub={nerdHub} displayPreferences={displayPreferences} />);
    expect(screen.getByText("STATS FOR NERDS")).toBeInTheDocument();
    expect(screen.getByText("2 of 4 connected")).toBeInTheDocument();

    // Every device that has one gets a row, with its own link numbers.
    expect(
      [...container.querySelectorAll("[data-nerd-device]")].map((row) =>
        row.getAttribute("data-nerd-device"),
      ),
    ).toEqual(["trainer", "heartRate", "power"]);
    const trainerRow = container.querySelector('[data-nerd-device="trainer"]')!;
    expect(trainerRow).toHaveTextContent("KICKR CORE");
    expect(trainerRow).toHaveTextContent("Wahoo · KICKR CORE");
    expect(trainerRow).toHaveTextContent("fw 1.2.3");
    expect(trainerRow).toHaveTextContent("200 W · 88 rpm");
    expect(trainerRow).toHaveTextContent("2.0 Hz");
    expect(trainerRow).toHaveTextContent("1200");
    expect(trainerRow).toHaveTextContent("44 02 c8 00");
    expect(trainerRow).toHaveTextContent("-55 dBm");
    expect(trainerRow).toHaveTextContent("feeding heart rate");
    expect(container.querySelector('[data-nerd-device="heartRate"]')).toHaveTextContent(
      "Link lost · reconnecting (attempt 2)",
    );
    expect(container.querySelector('[data-nerd-device="power"]')).toHaveTextContent("feeding power, cadence");
    expect(screen.getByText("Not connected: cadence sensor.")).toBeInTheDocument();

    // The fusion band names the source of each metric, fallback included.
    const fusion = container.querySelector(".nerd-fusion")!;
    expect(fusion).toHaveTextContent("POWER150 Wpower meter");
    expect(fusion).toHaveTextContent("HEART RATE140 bpmtrainer (fallback)");
    expect(fusion).toHaveTextContent("Power shown150 W");

    rerender(
      <Ride {...props} hub={nerdHub} displayPreferences={displayPreferences} powerSmoothing="3s" />,
    );
    expect(container.querySelector(".nerd-fusion")).toHaveTextContent(
      "Power shown150 W · 3 s avg · raw 150 W",
    );
  });

});
