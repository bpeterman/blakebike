import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("./api", () => ({
  api: {
    saveSourcePreferences: vi.fn(() => Promise.resolve()),
    disconnectDevice: vi.fn(() => Promise.resolve()),
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

    // Idle roles offer Connect.
    fireEvent.click(screen.getAllByRole("button", { name: /^Connect$/ })[0]);
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

  it("renders sensibly before the first snapshot arrives", () => {
    render(<DevicesPage hub={null} sources={undefined} onConnect={vi.fn()} perform={perform} />);
    expect(screen.getByText("0 of 4 connected")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^Connect$/ })).toHaveLength(4);
  });
});
