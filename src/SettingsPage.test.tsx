import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ComponentProps } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("./api", () => ({
  api: {
    appVersion: vi.fn(() => Promise.resolve("9.9.9")),
    openWebsite: vi.fn(() => Promise.resolve()),
    logFilePath: vi.fn(() => Promise.resolve("/tmp/blakebike.log")),
    rideFilesPath: vi.fn(() => Promise.resolve("/tmp/Ride Files")),
    intervalsApiKeyConfigured: vi.fn(() => Promise.resolve(false)),
    saveIntervalsApiKey: vi.fn(() => Promise.resolve()),
    clearIntervalsApiKey: vi.fn(() => Promise.resolve()),
    syncTrainingSettings: vi.fn(),
    saveTrainingZones: vi.fn(() => Promise.resolve()),
    revealRideFiles: vi.fn(() => Promise.resolve()),
    revealLogFile: vi.fn(() => Promise.resolve()),
  },
}));

import { api } from "./api";
import { SettingsPage } from "./App";
import { latestRelease } from "./releaseNotes";
import {
  defaultRideDisplayPreferences,
  defaultTrainingZoneSettings,
  derivedPowerZones,
  type Profile,
  type TrainingSyncResult,
  type TrainingZoneSettings,
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

const renderSettings = (
  overrides: Partial<ComponentProps<typeof SettingsPage>> = {},
) =>
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
      {...overrides}
    />,
  );

const powerToggle = () =>
  screen.getByRole("checkbox", { name: /Import power zones from Intervals\.icu/ });
const heartRateToggle = () =>
  screen.getByRole("checkbox", { name: /Import heart rate zones from Intervals\.icu/ });
const syncButton = () => screen.getByRole("button", { name: "Sync from Intervals.icu" });

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("training zone settings", () => {
  it("switches edited zones to custom and can reset them", () => {
    const onSaveTrainingZones = vi.fn();
    renderSettings({ onSaveTrainingZones });
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
  });

  it("has one independent import toggle per zone set, saved with the zone settings", () => {
    const onSaveTrainingZones = vi.fn();
    renderSettings({ onSaveTrainingZones });
    expect(powerToggle()).not.toBeChecked();
    expect(heartRateToggle()).not.toBeChecked();

    fireEvent.click(heartRateToggle());
    expect(heartRateToggle()).toBeChecked();
    expect(powerToggle()).not.toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "Save zone settings" }));
    expect(onSaveTrainingZones).toHaveBeenLastCalledWith(
      expect.objectContaining({
        syncPowerZonesFromIntervals: false,
        syncHeartRateZonesFromIntervals: true,
      }),
    );

    fireEvent.click(powerToggle());
    fireEvent.click(screen.getByRole("button", { name: "Save zone settings" }));
    expect(onSaveTrainingZones).toHaveBeenLastCalledWith(
      expect.objectContaining({
        syncPowerZonesFromIntervals: true,
        syncHeartRateZonesFromIntervals: true,
      }),
    );
  });

  it("asks before turning import on over custom zones and keeps them on cancel", () => {
    renderSettings();
    fireEvent.change(screen.getByLabelText("Heart rate zone 1 upper bound"), {
      target: { value: "121" },
    });
    fireEvent.click(heartRateToggle());
    expect(
      screen.getByRole("alertdialog", { name: "Replace your custom heart-rate zones?" }),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Keep it" }));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(heartRateToggle()).not.toBeChecked();

    fireEvent.click(heartRateToggle());
    fireEvent.click(screen.getByRole("button", { name: "Replace on sync" }));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(heartRateToggle()).toBeChecked();
    expect(
      screen.getByText(/the next sync replaces the boundaries you edited by hand/),
    ).toBeInTheDocument();
    // Derived zones have nothing to lose, so the power toggle flips without asking.
    fireEvent.click(powerToggle());
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(powerToggle()).toBeChecked();
  });

  it("labels imported zones and lets them go back to derived", () => {
    const onSaveTrainingZones = vi.fn();
    const imported: TrainingZoneSettings = {
      ...defaultTrainingZoneSettings,
      syncPowerZonesFromIntervals: true,
      powerMode: "intervals",
      powerZones: derivedPowerZones(250),
    };
    renderSettings({ trainingZones: imported, onSaveTrainingZones });
    expect(screen.getByText("From Intervals.icu")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Reset defaults" }));
    fireEvent.click(screen.getByRole("button", { name: "Save zone settings" }));
    expect(onSaveTrainingZones).toHaveBeenLastCalledWith(
      expect.objectContaining({ powerMode: "derived", powerZones: [] }),
    );
  });
});

describe("Intervals.icu settings", () => {
  it("saves and clears the API key without reading it back", async () => {
    renderSettings();

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
    expect(syncButton()).toBeDisabled();
  });

  it("syncs without saving drafts and reports every item", async () => {
    vi.mocked(api.intervalsApiKeyConfigured).mockResolvedValue(true);
    let resolveSync!: (result: TrainingSyncResult) => void;
    vi.mocked(api.syncTrainingSettings).mockReturnValue(
      new Promise((resolve) => {
        resolveSync = resolve;
      }),
    );
    const onProfileUpdate = vi.fn();
    const onTrainingZonesUpdate = vi.fn();
    renderSettings({ onProfileUpdate, onTrainingZonesUpdate });

    const sync = await screen.findByRole("button", { name: "Sync from Intervals.icu" });
    await waitFor(() => expect(sync).toBeEnabled());
    fireEvent.click(sync);
    expect(screen.getByRole("button", { name: "Syncing…" })).toBeDisabled();

    const updated = { ...profile, ftpWatts: 267, maxHeartRateBpm: 192 };
    const zones: TrainingZoneSettings = {
      ...defaultTrainingZoneSettings,
      syncPowerZonesFromIntervals: true,
      powerMode: "intervals",
      powerZones: derivedPowerZones(267),
    };
    resolveSync({
      profile: updated,
      zones,
      ftp: { watts: 267, previousWatts: 220, source: "indoorFtp" },
      maxHeartRate: { bpm: 192, previousBpm: 190 },
      powerZones: { status: "imported" },
      heartRateZones: { status: "syncOff" },
    });
    await waitFor(() => expect(onProfileUpdate).toHaveBeenCalledWith(updated));
    expect(onTrainingZonesUpdate).toHaveBeenCalledWith(zones);
    expect(api.saveTrainingZones).not.toHaveBeenCalled();
    expect(
      screen.getByText(
        "FTP 267 W from your Intervals.icu indoor FTP (was 220 W) · Max HR 192 bpm (was 190 bpm) · Power zones imported · Heart-rate zones not imported (import is off)",
      ),
    ).toBeInTheDocument();
    expect(syncButton()).toBeEnabled();
  });

  it("waits for unsaved profile or zone edits instead of saving them itself", async () => {
    vi.mocked(api.intervalsApiKeyConfigured).mockResolvedValue(true);
    renderSettings();
    const sync = await screen.findByRole("button", { name: "Sync from Intervals.icu" });
    await waitFor(() => expect(sync).toBeEnabled());

    fireEvent.change(screen.getByLabelText("Power zone 1 upper bound"), {
      target: { value: "125" },
    });
    expect(sync).toBeDisabled();
    expect(screen.getByText("Save your zone settings first.")).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Rider name"), { target: { value: "B" } });
    expect(
      screen.getByText("Save your rider profile and zone settings first."),
    ).toBeInTheDocument();

    fireEvent.click(screen.getAllByRole("button", { name: "Reset defaults" })[0]);
    expect(screen.getByText("Save your rider profile first.")).toBeInTheDocument();
    expect(api.syncTrainingSettings).not.toHaveBeenCalled();
    expect(api.saveTrainingZones).not.toHaveBeenCalled();
  });

  it("toggles developer mode", () => {
    const onDevMode = vi.fn();
    renderSettings({ onDevMode });

    const toggle = screen.getByRole("checkbox", {
      name: /Offer simulated devices/,
    });
    expect(toggle).not.toBeChecked();
    fireEvent.click(toggle);
    expect(onDevMode).toHaveBeenCalledWith(true);
  });

  it("drafts, reorders, resets, and saves the ride layout", () => {
    const onSaveRideDisplayPreferences = vi.fn();
    renderSettings({ onSaveRideDisplayPreferences });

    // Stats for nerds is in the list but off until the rider asks for it.
    const nerdStats = screen.getByRole("checkbox", { name: "Stats for nerds" });
    expect(nerdStats).not.toBeChecked();
    fireEvent.click(nerdStats);
    fireEvent.click(screen.getByRole("checkbox", { name: "Power" }));
    fireEvent.click(screen.getByRole("button", { name: "Move Cadence up" }));
    expect(onSaveRideDisplayPreferences).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Save ride layout" }));
    expect(onSaveRideDisplayPreferences).toHaveBeenLastCalledWith(
      expect.objectContaining({
        version: 2,
        cards: expect.arrayContaining([
          { id: "power", visible: false },
          { id: "deviceStats", visible: true },
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

describe("about card", () => {
  it("shows the running version with only the latest summary, not every note", async () => {
    renderSettings();

    await screen.findByText(/blake\.bike 9\.9\.9/);
    expect(screen.getByText(latestRelease!.summary)).toBeInTheDocument();
    const firstItem = latestRelease!.sections[0]?.items[0];
    expect(firstItem).toBeDefined();
    expect(screen.queryByText(firstItem!)).not.toBeInTheDocument();
  });

  it("opens the full release notes in a modal", async () => {
    renderSettings();

    fireEvent.click(await screen.findByRole("button", { name: /Release notes/ }));
    expect(screen.getByText("What's new in blake.bike")).toBeInTheDocument();
    expect(screen.getByText(latestRelease!.sections[0]!.items[0]!)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Close release notes" }));
    await waitFor(() =>
      expect(screen.queryByText("What's new in blake.bike")).not.toBeInTheDocument(),
    );
  });

  it("opens the developer website", async () => {
    renderSettings();

    fireEvent.click(await screen.findByRole("button", { name: /blake\.bike$/ }));
    expect(api.openWebsite).toHaveBeenCalled();
  });
});
