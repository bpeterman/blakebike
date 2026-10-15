import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RangeSummaryStrip, RideDetailModal } from "./App";
import type { RangeSummary } from "./chartRange";
import { defaultTrainingZoneSettings, type Profile, type SessionDetail } from "./types";

const summary: RangeSummary = {
  range: { startMs: 750_000, endMs: 1_050_000 },
  durationMs: 300_000,
  power: { average: 245, max: 310, min: 180, count: 300 },
  heartRate: { average: 152, max: 166, min: 140, count: 300 },
};

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

const session: SessionDetail = {
  summary: {
    id: "session",
    workoutId: null,
    workoutName: "Threshold",
    startedAt: "2026-09-19T12:00:00Z",
    endedAt: "2026-09-19T12:30:00Z",
    elapsedSeconds: 1800,
    averagePowerWatts: 205,
    maxPowerWatts: 410,
    averageCadenceRpm: 88,
    estimatedDistanceMeters: 15000,
    distanceSource: "trainer",
    distanceWeightKg: 84,
    completed: true,
  },
  samples: [
    { timestampMs: 1000, powerWatts: 205, cadenceRpm: 88, speedKph: 30, heartRateBpm: 150, targetPowerWatts: 200 },
  ],
};

afterEach(cleanup);

describe("RangeSummaryStrip", () => {
  it("shows a hint until a range is selected", () => {
    render(<RangeSummaryStrip summary={null} onClear={vi.fn()} />);
    expect(screen.getByText(/Drag across a chart/)).toBeInTheDocument();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("reports the span and both series, and clears on request", () => {
    const onClear = vi.fn();
    render(<RangeSummaryStrip summary={summary} onClear={onClear} />);
    const status = screen.getByRole("status");
    expect(status).toHaveTextContent("12:30 – 17:30");
    expect(status).toHaveTextContent("5:00 selected");
    expect(screen.getByText("W average").previousSibling).toHaveTextContent("245");
    expect(screen.getByText("W maximum").previousSibling).toHaveTextContent("310");
    expect(screen.getByText("W minimum").previousSibling).toHaveTextContent("180");
    expect(screen.getByText("bpm average").previousSibling).toHaveTextContent("152");
    expect(screen.getByText("bpm maximum").previousSibling).toHaveTextContent("166");
    expect(screen.getByText("bpm minimum").previousSibling).toHaveTextContent("140");
    fireEvent.click(screen.getByRole("button", { name: "Clear" }));
    expect(onClear).toHaveBeenCalledTimes(1);
  });

  it("shows dashes for a series with no samples in the range", () => {
    render(<RangeSummaryStrip summary={{ ...summary, heartRate: null }} onClear={vi.fn()} />);
    expect(screen.getByText("bpm average").previousSibling).toHaveTextContent("—");
    expect(screen.getByText("W average").previousSibling).toHaveTextContent("245");
  });
});

describe("ride detail chart selection", () => {
  it("starts without a selection and lets Escape close the modal", () => {
    const onClose = vi.fn();
    render(
      <RideDetailModal
        session={session}
        profile={profile}
        trainingZones={defaultTrainingZoneSettings}
        onClose={onClose}
        onExport={vi.fn()}
        onExportFit={vi.fn()}
        onGarmin={vi.fn()}
      />,
    );
    expect(screen.getByText(/Drag across a chart/)).toBeInTheDocument();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
