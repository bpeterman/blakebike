import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ComponentProps } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("./api", () => ({
  api: {
    appVersion: vi.fn(() => Promise.resolve("9.9.9")),
    openWebsite: vi.fn(() => Promise.resolve()),
    logFilePath: vi.fn(() => Promise.resolve("/tmp/blakebike.log")),
    rideFilesPath: vi.fn(() => Promise.resolve("/tmp/Ride Files")),
    syncTrainingSettings: vi.fn(),
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
  derivedPowerZones,
  disconnectedIntervalsStatus,
  type IntervalsStatus,
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

const connectedStatus: IntervalsStatus = {
  ...disconnectedIntervalsStatus,
  configured: true,
  athleteId: "i1",
  athleteName: "Blake P",
};

const settingsElement = (
  overrides: Partial<ComponentProps<typeof SettingsPage>> = {},
) => (
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
  />
);

const renderSettings = (
  overrides: Partial<ComponentProps<typeof SettingsPage>> = {},
) => render(settingsElement(overrides));

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
  it("hands the API key to App and never reads it back", async () => {
    const onSaveIntervalsKey = vi.fn(() => Promise.resolve());
    const onClearIntervalsKey = vi.fn(() => Promise.resolve());
    const { rerender } = renderSettings({ onSaveIntervalsKey, onClearIntervalsKey });

    expect(screen.getByText("API key not configured")).toBeInTheDocument();
    expect(syncButton()).toBeDisabled();
    expect(screen.getByRole("button", { name: "Sync now" })).toBeDisabled();
    fireEvent.change(screen.getByLabelText("API key"), {
      target: { value: "secret-key" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save API key" }));
    await waitFor(() => expect(onSaveIntervalsKey).toHaveBeenCalledWith("secret-key"));
    await waitFor(() => expect(screen.getByLabelText("API key")).toHaveValue(""));

    // App owns the status; once it reports the athlete the card follows.
    rerender(settingsElement({ onSaveIntervalsKey, onClearIntervalsKey, intervalsStatus: connectedStatus }));
    expect(screen.getByText("Connected as Blake P")).toBeInTheDocument();
    expect(syncButton()).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: "Clear key" }));
    await waitFor(() => expect(onClearIntervalsKey).toHaveBeenCalledOnce());
  });

  it("saves the mirror toggles at once and shows the last sync", async () => {
    const onIntervalsSyncSettings = vi.fn(() => Promise.resolve());
    renderSettings({
      intervalsStatus: {
        ...connectedStatus,
        lastSyncedAt: new Date(Date.now() - 2 * 3_600_000).toISOString(),
        lastError: "Could not reach Intervals.icu",
      },
      onIntervalsSyncSettings,
    });
    expect(screen.getByText(/Synced 2 hours ago\./)).toBeInTheDocument();
    expect(screen.getByText("Last sync failed: Could not reach Intervals.icu")).toBeInTheDocument();

    const library = screen.getByRole("checkbox", { name: /Mirror the workout library/ });
    const calendar = screen.getByRole("checkbox", { name: /Show today's planned workout/ });
    fireEvent.click(library);
    expect(onIntervalsSyncSettings).toHaveBeenCalledWith({ calendar: true, library: false });
    // One round trip at a time: the other toggle waits until this one lands.
    expect(calendar).toBeDisabled();
    await waitFor(() => expect(calendar).toBeEnabled());
    fireEvent.click(calendar);
    expect(onIntervalsSyncSettings).toHaveBeenLastCalledWith({ calendar: false, library: true });
  });

  it("runs Sync now through App and reports what changed", async () => {
    const onSyncIntervals = vi.fn(() =>
      Promise.resolve({
        syncedAt: "2026-09-21T06:00:00Z",
        calendar: { status: "done" as const, report: { fetched: 2, unstructured: 0, failed: 0, skipped: 0 } },
        library: { status: "done" as const, report: { added: 1, updated: 0, removed: 0, unchanged: 5, skipped: 0, failed: [] } },
      }),
    );
    renderSettings({ intervalsStatus: connectedStatus, onSyncIntervals });
    fireEvent.click(screen.getByRole("button", { name: "Sync now" }));
    expect(onSyncIntervals).toHaveBeenCalledOnce();
    expect(
      await screen.findByText("2 planned rides in the next 7 days · Library: 1 added"),
    ).toBeInTheDocument();
  });

  it("syncs without saving drafts and reports every item", async () => {
    let resolveSync!: (result: TrainingSyncResult) => void;
    vi.mocked(api.syncTrainingSettings).mockReturnValue(
      new Promise((resolve) => {
        resolveSync = resolve;
      }),
    );
    const onProfileUpdate = vi.fn();
    const onTrainingZonesUpdate = vi.fn();
    renderSettings({ onProfileUpdate, onTrainingZonesUpdate, intervalsStatus: connectedStatus });

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
    renderSettings({ intervalsStatus: connectedStatus });
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

  it("edits ride screens: fields, panels, order, size, and saving", () => {
    const onSaveRideDisplayPreferences = vi.fn();
    renderSettings({ onSaveRideDisplayPreferences });

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
