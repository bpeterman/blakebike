import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("./api", () => ({
  api: {
    logFilePath: vi.fn(() => Promise.resolve("/tmp/blakebike.log")),
    rideFilesPath: vi.fn(() => Promise.resolve("/tmp/Ride Files")),
    intervalsApiKeyConfigured: vi.fn(() => Promise.resolve(false)),
    saveIntervalsApiKey: vi.fn(() => Promise.resolve()),
    clearIntervalsApiKey: vi.fn(() => Promise.resolve()),
    refreshEstimatedFtp: vi.fn(),
    saveTrainingZones: vi.fn(() => Promise.resolve()),
    revealRideFiles: vi.fn(() => Promise.resolve()),
    revealLogFile: vi.fn(() => Promise.resolve()),
  },
}));

import { api } from "./api";
import { SettingsPage } from "./App";
import {
  defaultRideDisplayPreferences,
  defaultTrainingZoneSettings,
  type Profile,
} from "./types";

const profile: Profile = {
  id: "profile",
  name: "Blake",
  ftpWatts: 220,
  maxPowerWatts: 1000,
  maxHeartRateBpm: 190,
  riderWeightKg: 75,
  bikeWeightKg: 9,
  weightUnit: "kg",
  distanceUnit: "km",
};

const perform = async (action: () => Promise<unknown>) => {
  await action();
};

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("Intervals.icu settings", () => {
  it("switches edited zones to custom and can reset them", async () => {
    const onSaveTrainingZones = vi.fn();
    render(
      <SettingsPage
        profile={profile}
        trainingZones={defaultTrainingZoneSettings}
        rideDisplayPreferences={defaultRideDisplayPreferences}
        perform={perform}
        onProfileUpdate={vi.fn()}
        onTrainingZonesUpdate={vi.fn()}
        onSave={vi.fn()}
        onSaveTrainingZones={onSaveTrainingZones}
        onSaveRideDisplayPreferences={vi.fn()}
        devMode={false}
        onDevMode={vi.fn()}
        onForgetDevices={() => Promise.resolve()}
      />,
    );
    fireEvent.change(screen.getByLabelText("Power zone 1 upper bound"), {
      target: { value: "125" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save zone settings" }));
    expect(onSaveTrainingZones).toHaveBeenCalledWith(
      expect.objectContaining({
        powerMode: "custom",
        powerZones: expect.arrayContaining([
          expect.objectContaining({ upperBound: 125 }),
        ]),
      }),
    );
    fireEvent.click(screen.getAllByRole("button", { name: "Reset defaults" })[0]);
    fireEvent.click(screen.getByRole("button", { name: "Save zone settings" }));
    expect(onSaveTrainingZones).toHaveBeenLastCalledWith(
      expect.objectContaining({ powerMode: "derived", powerZones: [] }),
    );

    fireEvent.click(
      screen.getByRole("checkbox", {
        name: /Import power zones from Intervals\.icu/,
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Save zone settings" }));
    expect(onSaveTrainingZones).toHaveBeenLastCalledWith(
      expect.objectContaining({ syncPowerZonesFromIntervals: true }),
    );
  });

  it("saves and clears the API key without reading it back", async () => {
    render(
      <SettingsPage
        profile={profile}
        trainingZones={defaultTrainingZoneSettings}
        rideDisplayPreferences={defaultRideDisplayPreferences}
        perform={perform}
        onProfileUpdate={vi.fn()}
        onTrainingZonesUpdate={vi.fn()}
        onSave={vi.fn()}
        onSaveTrainingZones={vi.fn()}
        onSaveRideDisplayPreferences={vi.fn()}
        devMode={false}
        onDevMode={vi.fn()}
        onForgetDevices={() => Promise.resolve()}
      />,
    );

    await screen.findByText("API key not configured");
    fireEvent.change(screen.getByLabelText("API key"), {
      target: { value: "secret-key" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save API key" }));

    await screen.findByText("API key saved");
    expect(api.saveIntervalsApiKey).toHaveBeenCalledWith("secret-key");
    expect(screen.getByLabelText("API key")).toHaveValue("");

    fireEvent.click(screen.getByRole("button", { name: "Clear key" }));
    await screen.findByText("API key not configured");
    expect(api.clearIntervalsApiKey).toHaveBeenCalledOnce();
    expect(
      screen.getByRole("button", { name: "Refresh training settings" }),
    ).toBeDisabled();
  });

  it("shows refresh progress and publishes the updated profile", async () => {
    vi.mocked(api.intervalsApiKeyConfigured).mockResolvedValue(true);
    let resolveRefresh!: (result: {
      profile: Profile;
      zones: typeof defaultTrainingZoneSettings;
      powerZonesImported: boolean;
      heartRateZonesImported: boolean;
    }) => void;
    vi.mocked(api.refreshEstimatedFtp).mockReturnValue(
      new Promise((resolve) => {
        resolveRefresh = resolve;
      }),
    );
    const onProfileUpdate = vi.fn();

    render(
      <SettingsPage
        profile={profile}
        trainingZones={defaultTrainingZoneSettings}
        rideDisplayPreferences={defaultRideDisplayPreferences}
        perform={perform}
        onProfileUpdate={onProfileUpdate}
        onTrainingZonesUpdate={vi.fn()}
        onSave={vi.fn()}
        onSaveTrainingZones={vi.fn()}
        onSaveRideDisplayPreferences={vi.fn()}
        devMode={false}
        onDevMode={vi.fn()}
        onForgetDevices={() => Promise.resolve()}
      />,
    );

    const refresh = await screen.findByRole("button", {
      name: "Refresh training settings",
    });
    await waitFor(() => expect(refresh).toBeEnabled());
    fireEvent.click(refresh);
    expect(
      screen.getByRole("button", { name: "Refreshing…" }),
    ).toBeDisabled();

    const updated = { ...profile, ftpWatts: 267 };
    resolveRefresh({
      profile: updated,
      zones: defaultTrainingZoneSettings,
      powerZonesImported: true,
      heartRateZonesImported: true,
    });
    await waitFor(() => expect(onProfileUpdate).toHaveBeenCalledWith(updated));
    expect(api.saveTrainingZones).toHaveBeenCalledWith(defaultTrainingZoneSettings);
    expect(
      screen.getByText("FTP, heart-rate zones, and power zones updated."),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Refresh training settings" }),
    ).toBeEnabled();
  });

  it("toggles developer mode", () => {
    const onDevMode = vi.fn();
    render(
      <SettingsPage
        profile={profile}
        trainingZones={defaultTrainingZoneSettings}
        rideDisplayPreferences={defaultRideDisplayPreferences}
        perform={perform}
        onProfileUpdate={vi.fn()}
        onTrainingZonesUpdate={vi.fn()}
        onSave={vi.fn()}
        onSaveTrainingZones={vi.fn()}
        onSaveRideDisplayPreferences={vi.fn()}
        devMode={false}
        onDevMode={onDevMode}
        onForgetDevices={() => Promise.resolve()}
      />,
    );

    const toggle = screen.getByRole("checkbox", {
      name: /Offer simulated devices/,
    });
    expect(toggle).not.toBeChecked();
    fireEvent.click(toggle);
    expect(onDevMode).toHaveBeenCalledWith(true);
  });

  it("drafts, reorders, resets, and saves the ride layout", () => {
    const onSaveRideDisplayPreferences = vi.fn();
    render(
      <SettingsPage
        profile={profile}
        trainingZones={defaultTrainingZoneSettings}
        rideDisplayPreferences={defaultRideDisplayPreferences}
        perform={perform}
        onProfileUpdate={vi.fn()}
        onTrainingZonesUpdate={vi.fn()}
        onSave={vi.fn()}
        onSaveTrainingZones={vi.fn()}
        onSaveRideDisplayPreferences={onSaveRideDisplayPreferences}
        devMode={false}
        onDevMode={vi.fn()}
        onForgetDevices={() => Promise.resolve()}
      />,
    );

    fireEvent.click(screen.getByRole("checkbox", { name: "Power" }));
    fireEvent.click(screen.getByRole("button", { name: "Move Cadence up" }));
    expect(onSaveRideDisplayPreferences).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Save ride layout" }));
    expect(onSaveRideDisplayPreferences).toHaveBeenLastCalledWith(
      expect.objectContaining({
        version: 2,
        cards: expect.arrayContaining([
          { id: "power", visible: false },
        ]),
      }),
    );
    expect(onSaveRideDisplayPreferences.mock.calls[0][0].cards[0].id).toBe(
      "cadence",
    );

    fireEvent.click(screen.getByRole("button", { name: "Reset to default" }));
    fireEvent.click(screen.getByRole("button", { name: "Save ride layout" }));
    expect(onSaveRideDisplayPreferences.mock.calls[1][0]).toEqual(
      defaultRideDisplayPreferences,
    );
  });
});
