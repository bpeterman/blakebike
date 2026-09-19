import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  PostRidePrompt,
  RideDetailModal,
} from "./App";
import { shouldPromptForPostRide } from "./postRide";
import {
  defaultTrainingZoneSettings,
  type Profile,
  type RunnerState,
  type SessionDetail,
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

const running: RunnerState = {
  status: "running",
  sessionId: "session",
  workoutName: "Threshold",
  elapsedSeconds: 60,
  totalSeconds: 60,
  intervalIndex: 0,
  intervalElapsedSeconds: 60,
  targetPowerWatts: 200,
  plannedTargetWatts: 200,
  manualErg: false,
  overrideActive: false,
  biasPercent: 100,
};

const session: SessionDetail = {
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
    distanceSource: "trainer",
    distanceWeightKg: 84,
    completed: true,
  },
  samples: [
    {
      timestampMs: 1000,
      powerWatts: 205,
      cadenceRpm: 88,
      speedKph: 30,
      heartRateBpm: 150,
      targetPowerWatts: 200,
    },
  ],
};

afterEach(cleanup);

describe("post-ride flow", () => {
  it("only prompts for a saved ride after an active ride ends", () => {
    expect(
      shouldPromptForPostRide(running, {
        status: "finished",
        sessionId: "session",
        completed: true,
      }),
    ).toBe(true);
    expect(
      shouldPromptForPostRide(running, {
        status: "error",
        message: "Trainer disconnected",
        sessionId: "session",
      }),
    ).toBe(true);
    expect(
      shouldPromptForPostRide({ status: "idle" }, {
        status: "finished",
        sessionId: "old-session",
        completed: true,
      }),
    ).toBe(false);
    expect(
      shouldPromptForPostRide(running, {
        status: "error",
        message: "Could not start",
        sessionId: null,
      }),
    ).toBe(false);
  });

  it("offers review and dismiss actions", () => {
    const onView = vi.fn();
    const onDismiss = vi.fn();
    render(
      <PostRidePrompt
        errorMessage={null}
        onView={onView}
        onDismiss={onDismiss}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "View ride metrics" }));
    fireEvent.click(screen.getByRole("button", { name: "Dismiss ride summary" }));
    expect(onView).toHaveBeenCalledOnce();
    expect(onDismiss).toHaveBeenCalledOnce();
  });

  it("places upload and export actions before summary metrics", () => {
    const onGarmin = vi.fn();
    const onExportFit = vi.fn();
    const onExport = vi.fn();
    render(
      <RideDetailModal
        session={session}
        profile={profile}
        trainingZones={defaultTrainingZoneSettings}
        onClose={vi.fn()}
        onGarmin={onGarmin}
        onExportFit={onExportFit}
        onExport={onExport}
      />,
    );

    const upload = screen.getByRole("button", { name: "Upload to Garmin" });
    const metrics = screen.getByText("W average").closest(".detail-metrics");
    const position = upload
      .closest(".detail-action-block")
      ?.compareDocumentPosition(metrics as Node);
    expect((position ?? 0) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);

    fireEvent.click(upload);
    fireEvent.click(screen.getByRole("button", { name: "Export FIT" }));
    fireEvent.click(screen.getByRole("button", { name: "Export CSV" }));
    expect(onGarmin).toHaveBeenCalledWith(session.summary);
    expect(onExportFit).toHaveBeenCalledWith(session.summary);
    expect(onExport).toHaveBeenCalledWith(session.summary);
  });

  it("never claims a ride was saved when finalization reports a problem", () => {
    render(<PostRidePrompt errorMessage={null} saveWarning="No ride measurements were saved."
      onView={vi.fn()} onDismiss={vi.fn()} />);
    expect(screen.getByRole("alert")).toHaveTextContent("Ride ended with a save problem");
    expect(screen.getByRole("alert")).toHaveTextContent("No ride measurements were saved.");
    expect(screen.queryByText("Ride saved")).not.toBeInTheDocument();
  });

  it("keeps a recording warning visible when a saved ride is opened from history", () => {
    render(<RideDetailModal session={{ ...session, summary: { ...session.summary, recordingWarning: "Recording is incomplete." } }}
      profile={profile} trainingZones={defaultTrainingZoneSettings} onClose={vi.fn()}
      onExport={vi.fn()} onExportFit={vi.fn()} onGarmin={vi.fn()} />);
    expect(screen.getByRole("alert")).toHaveTextContent("Recording is incomplete.");
  });

});
