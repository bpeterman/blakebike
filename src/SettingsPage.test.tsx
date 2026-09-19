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
    revealRideFiles: vi.fn(() => Promise.resolve()),
    revealLogFile: vi.fn(() => Promise.resolve()),
  },
}));

import { api } from "./api";
import { SettingsPage } from "./App";
import type { Profile } from "./types";

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
  it("saves and clears the API key without reading it back", async () => {
    render(
      <SettingsPage
        profile={profile}
        perform={perform}
        onProfileUpdate={vi.fn()}
        onSave={vi.fn()}
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
      screen.getByRole("button", { name: "Refresh estimated FTP" }),
    ).toBeDisabled();
  });

  it("shows refresh progress and publishes the updated profile", async () => {
    vi.mocked(api.intervalsApiKeyConfigured).mockResolvedValue(true);
    let resolveRefresh!: (profile: Profile) => void;
    vi.mocked(api.refreshEstimatedFtp).mockReturnValue(
      new Promise((resolve) => {
        resolveRefresh = resolve;
      }),
    );
    const onProfileUpdate = vi.fn();

    render(
      <SettingsPage
        profile={profile}
        perform={perform}
        onProfileUpdate={onProfileUpdate}
        onSave={vi.fn()}
        onForgetDevices={() => Promise.resolve()}
      />,
    );

    const refresh = await screen.findByRole("button", {
      name: "Refresh estimated FTP",
    });
    await waitFor(() => expect(refresh).toBeEnabled());
    fireEvent.click(refresh);
    expect(
      screen.getByRole("button", { name: "Refreshing…" }),
    ).toBeDisabled();

    const updated = { ...profile, ftpWatts: 267 };
    resolveRefresh(updated);
    await waitFor(() => expect(onProfileUpdate).toHaveBeenCalledWith(updated));
    expect(
      screen.getByRole("button", { name: "Refresh estimated FTP" }),
    ).toBeEnabled();
  });
});
