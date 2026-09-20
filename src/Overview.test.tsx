import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ComponentProps } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("./api", () => ({ api: {} }));

import { Overview } from "./App";
import {
  derivedPowerZones,
  disconnectedIntervalsStatus,
  type IntervalsStatus,
  type PlannedWorkout,
  type Profile,
  type Workout,
} from "./types";

const profile: Profile = {
  id: "profile",
  name: "Blake",
  ftpWatts: 250,
  maxPowerWatts: 1000,
  maxHeartRateBpm: 190,
  riderWeightKg: 75,
  bikeWeightKg: 9,
  weightUnit: "kg",
  distanceUnit: "km",
};

const structured: Workout = {
  id: "11111111-1111-5111-8111-111111111111",
  name: "Sweet Spot 3x12",
  description: "",
  source: "intervals",
  version: 1,
  createdAt: "2026-09-21T00:00:00Z",
  updatedAt: "2026-09-21T00:00:00Z",
  steps: [
    { kind: "ramp", durationSeconds: 600, start: { unit: "percentFtp", value: 50 }, end: { unit: "percentFtp", value: 70 } },
    { kind: "steady", durationSeconds: 720, target: { unit: "percentFtp", value: 90 } },
  ],
};

const planned = (overrides: Partial<PlannedWorkout> = {}): PlannedWorkout => ({
  eventId: 501,
  workoutId: structured.id,
  date: "2026-09-21",
  name: "Sweet Spot 3x12",
  description: "Three twelve-minute blocks just under threshold.",
  activityType: "Ride",
  plannedLoad: 68,
  durationSeconds: 1320,
  workout: structured,
  parseError: null,
  updated: null,
  fetchedAt: "2026-09-21T06:00:00Z",
  ...overrides,
});

const connectedStatus: IntervalsStatus = {
  ...disconnectedIntervalsStatus,
  configured: true,
  athleteId: "i1",
  athleteName: "Blake P",
  lastSyncedAt: new Date().toISOString(),
};

const renderOverview = (overrides: Partial<ComponentProps<typeof Overview>> = {}) =>
  render(
    <Overview
      profile={profile}
      powerZones={derivedPowerZones(250)}
      workouts={[]}
      sessions={[]}
      connected
      devMode={false}
      onConnect={vi.fn()}
      onRide={vi.fn()}
      onNavigate={vi.fn()}
      onSyncTrainingSettings={() => Promise.resolve()}
      {...overrides}
    />,
  );

afterEach(cleanup);

describe("today's plan on the home screen", () => {
  it("starts a structured plan through the runner id", () => {
    const onStartPlanned = vi.fn();
    renderOverview({ intervalsStatus: connectedStatus, plannedToday: [planned()], onStartPlanned });
    const card = screen.getByRole("region", { name: "Today's plan" });
    expect(card).toHaveTextContent("Sweet Spot 3x12");
    expect(card).toHaveTextContent("68");
    expect(card).toHaveTextContent("planned load");
    expect(card).toHaveTextContent("22:00");
    fireEvent.click(screen.getByRole("button", { name: /^Start$/ }));
    expect(onStartPlanned).toHaveBeenCalledWith(structured.id);
  });

  it("offers a free ride for a plan without structure and explains an unreadable one", () => {
    const onStartFreeRide = vi.fn();
    renderOverview({
      intervalsStatus: connectedStatus,
      plannedToday: [planned({ workout: null, durationSeconds: 2700, plannedLoad: 25 })],
      onStartFreeRide,
    });
    expect(screen.getByText(/no structured steps on Intervals\.icu/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Start free ride" }));
    expect(onStartFreeRide).toHaveBeenCalledOnce();
    cleanup();

    renderOverview({
      intervalsStatus: connectedStatus,
      connected: false,
      plannedToday: [planned({ workout: null, parseError: "Unsupported workout element <Sprint>" })],
    });
    expect(screen.getByText(/could not read this workout's structure \(Unsupported workout element <Sprint>\)/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Set up free ride" })).toBeInTheDocument();
  });

  it("says when nothing is planned and when the cache is stale", () => {
    renderOverview({
      intervalsStatus: { ...connectedStatus, lastSyncedAt: new Date(Date.now() - 3 * 3_600_000).toISOString(), lastError: "Could not reach Intervals.icu" },
      plannedToday: [],
    });
    expect(screen.getByText("Nothing planned today")).toBeInTheDocument();
    expect(screen.getByText("Synced 3 hours ago · Intervals.icu unreachable")).toBeInTheDocument();
  });

  it("refreshes the plan from its own button", async () => {
    const onSyncIntervals = vi.fn(() => Promise.resolve());
    renderOverview({ intervalsStatus: connectedStatus, plannedToday: [planned()], onSyncIntervals });
    fireEvent.click(screen.getByRole("button", { name: "Refresh plan" }));
    expect(onSyncIntervals).toHaveBeenCalledOnce();
  });

  it("stays out of the way without a key or with the calendar off", () => {
    renderOverview({ plannedToday: [planned()] });
    expect(screen.queryByRole("region", { name: "Today's plan" })).not.toBeInTheDocument();
    cleanup();
    renderOverview({
      intervalsStatus: { ...connectedStatus, settings: { calendar: false, library: true } },
      plannedToday: [planned()],
    });
    expect(screen.queryByRole("region", { name: "Today's plan" })).not.toBeInTheDocument();
  });
});
