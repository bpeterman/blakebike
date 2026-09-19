import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const knownDevices = [
  {
    id: "k",
    name: "KICKR CORE",
    role: "trainer" as const,
    capabilities: ["ftms" as const],
    simulated: false,
    manufacturer: "Wahoo",
    model: "KICKR CORE",
    lastConnectedAt: new Date().toISOString(),
  },
  {
    id: "strap",
    name: "HRM-Pro",
    role: "heartRate" as const,
    capabilities: ["heartRate" as const],
    simulated: false,
    manufacturer: "Garmin",
    model: null,
    lastConnectedAt: new Date(Date.now() - 2 * 86_400_000).toISOString(),
  },
];

vi.mock("./api", () => ({
  api: {
    saveSourcePreferences: vi.fn(() => Promise.resolve()),
    disconnectDevice: vi.fn(() => Promise.resolve()),
    connectDevice: vi.fn(() => Promise.resolve()),
    knownDevices: vi.fn(() => Promise.resolve(knownDevices)),
    forgetDevice: vi.fn(() => Promise.resolve()),
  },
}));

import { api } from "./api";
import { DevicesPage } from "./DevicesPage";
import type { DeviceSlot, DevicesSnapshot } from "./types";

const idleStats: DeviceSlot["stats"] = {
  samples: 0,
  parseFailures: 0,
  lastSampleMs: null,
  rateHz: 0,
  rssi: null,
  batteryPercent: null,
  manufacturer: null,
  model: null,
  firmware: null,
  connectedSinceMs: null,
  drops: 0,
  lastRawHex: null,
  lastReading: null,
};

const snapshot: DevicesSnapshot = {
  scanning: false,
  scanError: null,
  sourcePreferences: {
    power: { mode: "auto" },
    cadence: { mode: "auto" },
    heartRate: { mode: "role", role: "heartRate" },
  },
  sources: {
    power: { role: "trainer", fallback: false },
    cadence: { role: "trainer", fallback: false },
    heartRate: { role: "trainer", fallback: true },
  },
  slots: [
    {
      role: "trainer",
      state: {
        status: "controlling",
        device: { id: "k", name: "KICKR CORE", simulated: false, rssi: -55, capabilities: ["ftms"] },
      },
      stats: {
        ...idleStats,
        samples: 1200,
        lastSampleMs: Date.now() - 400,
        rateHz: 2.0,
        rssi: -55,
        batteryPercent: null,
        manufacturer: "Wahoo",
        model: "KICKR CORE",
        firmware: "1.2.3",
        connectedSinceMs: Date.now() - 125_000,
        lastRawHex: "44 02 c8 00",
        lastReading: "200 W · 88 rpm · 30.1 km/h · target 200 W",
      },
      log: [
        { atMs: Date.now() - 3000, level: "ok", step: "Control granted", detail: "ERG mode available", connect: true },
        { atMs: Date.now() - 2000, level: "ok", step: "First sample received", detail: "198 W · 87 rpm", connect: false },
      ],
    },
    { role: "heartRate", state: { status: "reconnecting", name: "HRM-Pro" }, stats: { ...idleStats, drops: 1 }, log: [] },
    { role: "power", state: { status: "idle" }, stats: idleStats, log: [] },
    { role: "cadence", state: { status: "idle" }, stats: idleStats, log: [] },
  ],
};

const perform = async (action: () => Promise<unknown>) => {
  await action();
};

describe("DevicesPage", () => {
  afterEach(cleanup);

  it("renders a card per role with live stats and the log on demand", () => {
    const onConnect = vi.fn();
    render(<DevicesPage hub={snapshot} sources={snapshot.sources} onConnect={onConnect} perform={perform} />);

    expect(screen.getByText("1 of 4 connected")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "KICKR CORE" })).toBeInTheDocument();
    expect(screen.getByText("ERG control")).toBeInTheDocument();
    // Manufacturer shows under the connected device's name.
    expect(screen.getByText("Wahoo · KICKR CORE")).toBeInTheDocument();
    expect(screen.getByText("200 W · 88 rpm · 30.1 km/h · target 200 W")).toBeInTheDocument();
    expect(screen.getByText(/2\.0 Hz/)).toBeInTheDocument();
    expect(screen.getByText("-55 dBm")).toBeInTheDocument();
    expect(screen.getByText("2m 05s")).toBeInTheDocument();
    expect(screen.getByText(/feeding power, cadence, heart rate/i)).toBeInTheDocument();

    // The HR strap dropped: shows its name, the link-lost chip and a reconnect button.
    expect(screen.getByRole("heading", { name: "HRM-Pro" })).toBeInTheDocument();
    expect(screen.getByText("Link lost")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Reconnect/ }));
    expect(onConnect).toHaveBeenCalledWith("heartRate");

    // Idle roles offer Connect (the known-device row, when loaded, comes first).
    const connects = screen.getAllByRole("button", { name: /^Connect$/ });
    fireEvent.click(connects[connects.length - 2]);
    expect(onConnect).toHaveBeenCalledWith("power");

    // Expanding the trainer log shows device details and every line.
    fireEvent.click(screen.getAllByRole("button", { name: /Log/ })[0]);
    expect(screen.getByText("Wahoo")).toBeInTheDocument();
    expect(screen.getByText("44 02 c8 00")).toBeInTheDocument();
    expect(screen.getByText("Control granted")).toBeInTheDocument();
    expect(screen.getByText("First sample received")).toBeInTheDocument();
  });

  it("shows source selection, fallback state, and saves changes", () => {
    render(<DevicesPage hub={snapshot} sources={snapshot.sources} onConnect={vi.fn()} perform={perform} />);
    // Heart rate is pinned to the strap, which is down, so the trainer is a fallback.
    expect(screen.getByText("falling back to KICKR CORE")).toBeInTheDocument();
    expect(screen.getAllByText("from KICKR CORE")).toHaveLength(2);

    const selects = screen.getAllByRole("combobox");
    expect((selects[2] as HTMLSelectElement).value).toBe("heartRate");
    fireEvent.change(selects[0], { target: { value: "power" } });
    expect(api.saveSourcePreferences).toHaveBeenCalledWith({
      power: { mode: "role", role: "power" },
      cadence: { mode: "auto" },
      heartRate: { mode: "role", role: "heartRate" },
    });
  });

  it("lists known devices with make, offers one-click connect and forget", async () => {
    render(<DevicesPage hub={snapshot} sources={snapshot.sources} onConnect={vi.fn()} perform={perform} />);
    await waitFor(() => expect(screen.getByText("Connect again with one click")).toBeInTheDocument());
    // The trainer is connected right now; the strap was used two days ago.
    expect(screen.getByText("connected now")).toBeInTheDocument();
    expect(screen.getByText(/last used 2 days ago/)).toBeInTheDocument();
    expect(screen.getByText(/Heart rate · Garmin/)).toBeInTheDocument();

    const rows = screen.getAllByRole("button", { name: /^Connect$/ });
    // Two idle role cards plus the remembered strap row.
    expect(rows).toHaveLength(3);
    fireEvent.click(rows[0]);
    expect(api.connectDevice).toHaveBeenCalledWith("heartRate", expect.objectContaining({ id: "strap", name: "HRM-Pro" }));

    fireEvent.click(screen.getByRole("button", { name: "Forget HRM-Pro" }));
    await waitFor(() => expect(api.forgetDevice).toHaveBeenCalledWith("strap"));
  });

  it("renders sensibly before the first snapshot arrives", () => {
    render(<DevicesPage hub={null} sources={undefined} onConnect={vi.fn()} perform={perform} />);
    expect(screen.getByText("0 of 4 connected")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^Connect$/ }).length).toBeGreaterThanOrEqual(4);
  });
});
