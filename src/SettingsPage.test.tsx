import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
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
    refreshEstimatedFtp: vi.fn(),
    saveTrainingZones: vi.fn(() => Promise.resolve()),
    revealRideFiles: vi.fn(() => Promise.resolve()),
    revealLogFile: vi.fn(() => Promise.resolve()),
  },
}));

import { api } from "./api";
import { defaultRideDisplayPreferences } from "./rideScreens";
import { SettingsPage } from "./App";
import { latestRelease } from "./releaseNotes";
import {
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

  it("edits ride screens: fields, panels, order, size, and saving", () => {
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

    const first = screen.getByLabelText("Name of screen 1") as HTMLInputElement;
    expect(first.value).toBe("Ride");
    fireEvent.change(first, { target: { value: "Main" } });
    // Editing the draft does not save anything by itself.
    expect(onSaveRideDisplayPreferences).not.toHaveBeenCalled();

    // The first slot is power; make it full width and move it down one.
    const metrics = screen.getAllByLabelText("Metric") as HTMLSelectElement[];
    expect(metrics[0].value).toBe("power");
    const sizes = screen.getAllByLabelText("Size") as HTMLSelectElement[];
    fireEvent.change(sizes[0], { target: { value: "4" } });
    fireEvent.click(screen.getByRole("button", { name: "Move slot 1 of Main down" }));

    fireEvent.click(screen.getByRole("button", { name: "Save ride layout" }));
    const saved = onSaveRideDisplayPreferences.mock.calls[0][0];
    expect(saved.version).toBe(3);
    expect(saved.screens[0].name).toBe("Main");
    expect(saved.screens[0].items[0]).toEqual({
      kind: "field",
      field: { metric: "cadence", scope: "ride", aggregate: "current" },
      span: 1,
    });
    expect(saved.screens[0].items[1]).toEqual({
      kind: "field",
      field: { metric: "power", scope: "ride", aggregate: "current" },
      span: 4,
    });
  });

  it("only offers scopes and aggregates the chosen metric has", () => {
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
    const options = (select: HTMLSelectElement) =>
      [...select.options].map((option) => option.value);

    const scope = screen.getAllByLabelText("Measured over")[0] as HTMLSelectElement;
    const shownAs = screen.getAllByLabelText("Shown as")[0] as HTMLSelectElement;
    expect(options(scope)).toEqual(["ride", "interval"]);
    expect(options(shownAs)).toEqual(["current", "average", "max"]);

    // The block has no live reading of its own, so the aggregate follows.
    fireEvent.change(scope, { target: { value: "interval" } });
    expect(options(screen.getAllByLabelText("Shown as")[0] as HTMLSelectElement)).toEqual([
      "average",
      "max",
    ]);

    // Distance is only ever a whole-ride total, so both pickers lock.
    fireEvent.change(screen.getAllByLabelText("Metric")[0], { target: { value: "distance" } });
    expect(screen.getAllByLabelText("Measured over")[0]).toBeDisabled();
    expect(screen.getAllByLabelText("Shown as")[0]).toBeDisabled();
  });

  it("adds and removes screens, but never the last one", () => {
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
    fireEvent.click(screen.getByRole("button", { name: "Add screen" }));
    expect(screen.getByLabelText("Name of screen 3")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Remove Screen 3" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove Detail" }));
    expect(screen.queryByLabelText("Name of screen 2")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove Ride" })).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "Reset to default" }));
    fireEvent.click(screen.getByRole("button", { name: "Save ride layout" }));
    expect(onSaveRideDisplayPreferences).toHaveBeenLastCalledWith(defaultRideDisplayPreferences);
  });
});

describe("about card", () => {
  const renderSettings = () =>
    render(
      <SettingsPage
        profile={profile}
        trainingZones={defaultTrainingZoneSettings}
        rideDisplayPreferences={defaultRideDisplayPreferences}
        devMode={false}
        onDevMode={vi.fn()}
        perform={perform}
        onProfileUpdate={vi.fn()}
        onTrainingZonesUpdate={vi.fn()}
        onSave={vi.fn()}
        onSaveTrainingZones={vi.fn()}
        onSaveRideDisplayPreferences={vi.fn()}
        onForgetDevices={() => Promise.resolve()}
      />,
    );

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
