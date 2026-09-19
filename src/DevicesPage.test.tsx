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
    connectKnownDevices: vi.fn(() => Promise.resolve([])),
    forgetDevice: vi.fn(() => Promise.resolve()),
    restoreKnownDevices: vi.fn(() => Promise.resolve()),
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
  batteryStatus: null,
  batteryVoltage: null,
  manufacturer: null,
  model: null,
  firmware: null,
  connectedSinceMs: null,
  drops: 0,
  lastRawHex: null,
  lastReading: null,
  calibrationSupported: false,
  calibrating: false,
};

const snapshot: DevicesSnapshot = {
  scanning: false,
  scanError: null,
  antAdapter: { status: "notAttached" },
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
        calibrationSupported: true,
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
    render(<DevicesPage hub={snapshot} sources={snapshot.sources} onConnect={onConnect} onCalibrate={vi.fn()} onSourcePreference={vi.fn()} perform={perform} />);

    expect(screen.getByText("1 of 4 connected")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "KICKR CORE" })).toBeInTheDocument();
    expect(screen.getByText("ERG control")).toBeInTheDocument();
    // Manufacturer shows under the connected device's name.
    expect(screen.getByText("BLE · Wahoo · KICKR CORE")).toBeInTheDocument();
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
    const onSourcePreference = vi.fn();
    render(<DevicesPage hub={snapshot} sources={snapshot.sources} onConnect={vi.fn()} onCalibrate={vi.fn()} onSourcePreference={onSourcePreference} perform={perform} />);
    // Heart rate is pinned to the strap, which is down, so the trainer is a fallback.
    expect(screen.getByText("falling back to KICKR CORE")).toBeInTheDocument();
    expect(screen.getAllByText("from KICKR CORE")).toHaveLength(2);

    const selects = screen.getAllByRole("combobox");
    expect((selects[2] as HTMLSelectElement).value).toBe("heartRate");
    fireEvent.change(selects[0], { target: { value: "power" } });
    expect(onSourcePreference).toHaveBeenCalledWith("power", { mode: "role", role: "power" });
  });

  it("offers calibration only for a ready trainer that advertises spin-down", () => {
    const onCalibrate = vi.fn();
    const trainer = snapshot.slots[0];
    const ready: DevicesSnapshot = {
      ...snapshot,
      slots: [
        {
          ...trainer,
          state: {
            status: "ready",
            device: { id: "k", name: "KICKR CORE", simulated: false, rssi: -55, capabilities: ["ftms"] },
          },
          stats: { ...trainer.stats, calibrationSupported: true },
        },
        ...snapshot.slots.slice(1),
      ],
    };
    const view = render(
      <DevicesPage hub={ready} sources={ready.sources} onConnect={vi.fn()} onCalibrate={onCalibrate} onSourcePreference={vi.fn()} perform={perform} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Calibrate" }));
    expect(onCalibrate).toHaveBeenCalledOnce();

    const unsupported: DevicesSnapshot = {
      ...ready,
      slots: ready.slots.map((slot) =>
        slot.role === "trainer"
          ? { ...slot, stats: { ...slot.stats, calibrationSupported: false } }
          : slot,
      ),
    };
    view.rerender(
      <DevicesPage hub={unsupported} sources={unsupported.sources} onConnect={vi.fn()} onCalibrate={onCalibrate} onSourcePreference={vi.fn()} perform={perform} />,
    );
    expect(screen.getByRole("button", { name: "Calibrate" })).toBeDisabled();
    expect(screen.getByText(/does not advertise FTMS spin-down/)).toBeInTheDocument();
  });

  it("offers zero offset on a connected power meter, with its last zero and the meter's own request", () => {
    const onCalibrate = vi.fn();
    const twoHoursAgo = new Date(Date.now() - 2 * 3_600_000).toISOString();
    const meter: DeviceSlot = {
      role: "power",
      state: { status: "ready", device: { id: "pm", name: "Assioma", simulated: false, rssi: -50, capabilities: ["cyclingPower"] } },
      stats: {
        ...idleStats,
        samples: 30,
        lastSampleMs: Date.now() - 300,
        rateHz: 1,
        rssi: -50,
        manufacturer: "Favero",
        connectedSinceMs: Date.now() - 30_000,
        lastReading: "0 W · 0 rpm",
        calibrationSupported: true,
        calibrationRequested: true,
        lastCalibration: { at: twoHoursAgo, kind: "zeroOffset", offsetRaw: 1023 },
      },
      log: [],
    };
    const withMeter: DevicesSnapshot = {
      ...snapshot,
      slots: snapshot.slots.map((slot) => (slot.role === "power" ? meter : slot)),
    };
    const view = render(
      <DevicesPage hub={withMeter} sources={withMeter.sources} onConnect={vi.fn()} onCalibrate={onCalibrate} onSourcePreference={vi.fn()} perform={perform} />,
    );
    expect(screen.getByText("Zeroed 2 h ago · offset 1023")).toBeInTheDocument();
    expect(screen.getByText("Meter requests zeroing")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Zero offset" }));
    expect(onCalibrate).toHaveBeenCalledWith("power");

    // During a ride the button is off with a reason; while zeroing it says so.
    view.rerender(
      <DevicesPage hub={withMeter} sources={withMeter.sources} onConnect={vi.fn()} onCalibrate={onCalibrate} onSourcePreference={vi.fn()} rideActive perform={perform} />,
    );
    expect(screen.getByRole("button", { name: "Zero offset" })).toBeDisabled();
    expect(screen.getByText("Zero offset is unavailable during a ride.")).toBeInTheDocument();

    const zeroing: DevicesSnapshot = {
      ...withMeter,
      slots: withMeter.slots.map((slot) =>
        slot.role === "power" ? { ...slot, stats: { ...slot.stats, calibrating: true } } : slot,
      ),
    };
    view.rerender(
      <DevicesPage hub={zeroing} sources={zeroing.sources} onConnect={vi.fn()} onCalibrate={onCalibrate} onSourcePreference={vi.fn()} perform={perform} />,
    );
    expect(screen.getByRole("button", { name: "Zeroing…" })).toBeDisabled();

    const unsupported: DevicesSnapshot = {
      ...withMeter,
      slots: withMeter.slots.map((slot) =>
        slot.role === "power" ? { ...slot, stats: { ...slot.stats, calibrationSupported: false, calibrationRequested: false } } : slot,
      ),
    };
    view.rerender(
      <DevicesPage hub={unsupported} sources={unsupported.sources} onConnect={vi.fn()} onCalibrate={onCalibrate} onSourcePreference={vi.fn()} perform={perform} />,
    );
    expect(screen.getByRole("button", { name: "Zero offset" })).toBeDisabled();
    expect(screen.getByText(/does not advertise offset compensation/)).toBeInTheDocument();
    // Heart rate and cadence cards never offer calibration.
    expect(screen.getAllByRole("button", { name: /Calibrate|Zero offset/ })).toHaveLength(2);
  });

  it("shows when a remembered device was last zeroed", async () => {
    vi.mocked(api.knownDevices).mockResolvedValueOnce([
      {
        ...knownDevices[1],
        id: "pm",
        name: "Assioma",
        role: "power",
        capabilities: ["cyclingPower"],
        manufacturer: "Favero",
        lastCalibration: { at: new Date(Date.now() - 3 * 86_400_000).toISOString(), kind: "zeroOffset", offsetRaw: 1019 },
      },
    ]);
    render(<DevicesPage hub={snapshot} sources={snapshot.sources} onConnect={vi.fn()} onCalibrate={vi.fn()} onSourcePreference={vi.fn()} perform={perform} />);
    expect(await screen.findByText(/Zeroed 3 days ago · offset 1019/)).toBeInTheDocument();
  });

  it("lists known devices with make, offers one-click connect and forget", async () => {
    const onOfferUndo = vi.fn();
    render(<DevicesPage hub={snapshot} sources={snapshot.sources} onConnect={vi.fn()} onCalibrate={vi.fn()} onSourcePreference={vi.fn()} onOfferUndo={onOfferUndo} perform={perform} />);
    await waitFor(() => expect(screen.getByText("Connect again with one click")).toBeInTheDocument());
    // The trainer is connected right now; the strap was used two days ago.
    expect(screen.getByText("connected now")).toBeInTheDocument();
    expect(screen.getByText(/last used 2 days ago/)).toBeInTheDocument();
    expect(screen.getByText(/Heart rate · BLE · Garmin/)).toBeInTheDocument();

    const rows = screen.getAllByRole("button", { name: /^Connect$/ });
    // Two idle role cards plus the remembered strap row.
    expect(rows).toHaveLength(3);
    fireEvent.click(rows[0]);
    expect(api.connectDevice).toHaveBeenCalledWith("heartRate", expect.objectContaining({ id: "strap", name: "HRM-Pro" }));

    fireEvent.click(screen.getByRole("button", { name: "Forget HRM-Pro" }));
    await waitFor(() => expect(api.forgetDevice).toHaveBeenCalledWith("strap"));
    expect(onOfferUndo).toHaveBeenCalledWith("“HRM-Pro” forgotten.", expect.any(Function));
    await onOfferUndo.mock.calls[0][1]();
    expect(api.restoreKnownDevices).toHaveBeenCalledWith([knownDevices[1]]);
  });

  it("connects every remembered device at once and reports a sleeper softly", async () => {
    vi.mocked(api.connectKnownDevices).mockResolvedValueOnce([
      { role: "trainer", name: "KICKR CORE", status: "skipped", error: null },
      { role: "heartRate", name: "HRM-Pro", status: "failed", error: "Device was not seen within 10s" },
    ]);
    const onNotice = vi.fn();
    const outcomes: unknown[] = [];
    const recordingPerform = async (action: () => Promise<unknown>) => {
      outcomes.push(await action());
    };
    render(<DevicesPage hub={snapshot} sources={snapshot.sources} onConnect={vi.fn()} onCalibrate={vi.fn()} onSourcePreference={vi.fn()} onNotice={onNotice} perform={recordingPerform} />);
    const button = await screen.findByRole("button", { name: "Connect all" });
    // The strap's role is free, so there is something to do.
    expect(button).toBeEnabled();
    fireEvent.click(button);
    await waitFor(() => expect(api.connectKnownDevices).toHaveBeenCalledOnce());
    await waitFor(() => expect(onNotice).toHaveBeenCalledWith(expect.stringContaining("Heart rate (HRM-Pro) didn’t answer")));
    // A soft outcome suppresses the generic success toast.
    expect(outcomes).toEqual([null]);
    expect(onNotice.mock.calls[0][0]).not.toContain("10s");
    await waitFor(() => expect(screen.getByRole("button", { name: "Connect all" })).toBeEnabled());
  });

  it("has nothing to connect when every remembered role is already taken", async () => {
    const allTaken: DevicesSnapshot = {
      ...snapshot,
      slots: snapshot.slots.map((slot) =>
        slot.role === "heartRate"
          ? { ...slot, state: { status: "ready", device: { id: "strap", name: "HRM-Pro", simulated: false, rssi: -60, capabilities: ["heartRate"] } } }
          : slot,
      ),
    };
    render(<DevicesPage hub={allTaken} sources={allTaken.sources} onConnect={vi.fn()} onCalibrate={vi.fn()} onSourcePreference={vi.fn()} perform={perform} />);
    expect(await screen.findByRole("button", { name: "Connect all" })).toBeDisabled();
  });

  it("shows the ANT adapter card at the bottom and expands the HR connect action only when ready", () => {
    const view = render(<DevicesPage hub={snapshot} sources={snapshot.sources} onConnect={vi.fn()} onCalibrate={vi.fn()} onSourcePreference={vi.fn()} perform={perform} />);
    expect(screen.queryByRole("heading", { name: "ANT+ receiver" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Reconnect/ })).toHaveAttribute("title", "Connect via Bluetooth");
    view.rerender(
      <DevicesPage
        hub={{ ...snapshot, antAdapter: { status: "permissionDenied", message: "Permission denied on /dev/ttyUSB0" } }}
        sources={snapshot.sources}
        onConnect={vi.fn()}
        onCalibrate={vi.fn()}
        onSourcePreference={vi.fn()}
        perform={perform}
      />,
    );
    expect(screen.getByRole("heading", { name: "ANT+ receiver" })).toBeInTheDocument();
    expect(screen.getByText(/Permission denied.*ttyUSB0/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Reconnect/ })).toHaveAttribute("title", "Connect via Bluetooth");

    view.rerender(
      <DevicesPage
        hub={{ ...snapshot, antAdapter: { status: "ready", name: "ANT USBStick2" } }}
        sources={snapshot.sources}
        onConnect={vi.fn()}
        onCalibrate={vi.fn()}
        onSourcePreference={vi.fn()}
        perform={perform}
      />,
    );
    const adapterHeading = screen.getByRole("heading", { name: "ANT+ receiver" });
    const sourcesHeading = screen.getByRole("heading", { name: "Which device feeds each metric" });
    expect(sourcesHeading.compareDocumentPosition(adapterHeading) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const reconnect = screen.getByRole("button", { name: /Reconnect/ });
    expect(reconnect).toHaveAttribute("title", "Connect via Bluetooth or ANT+");
    expect(reconnect.querySelector(".lucide-radio")).toBeInTheDocument();
  });

  it("shows ANT battery status when the sensor omits an exact percentage", () => {
    const batterySnapshot: DevicesSnapshot = {
      ...snapshot,
      slots: snapshot.slots.map((slot) =>
        slot.role === "heartRate"
          ? { ...slot, stats: { ...slot.stats, batteryStatus: "Good", batteryVoltage: 2.5 } }
          : slot,
      ),
    };
    render(<DevicesPage hub={batterySnapshot} sources={batterySnapshot.sources} onConnect={vi.fn()} onCalibrate={vi.fn()} onSourcePreference={vi.fn()} perform={perform} />);
    expect(screen.getByText("Good")).toBeInTheDocument();
  });

  it("renders sensibly before the first snapshot arrives", () => {
    render(<DevicesPage hub={null} sources={undefined} onConnect={vi.fn()} onCalibrate={vi.fn()} onSourcePreference={vi.fn()} perform={perform} />);
    expect(screen.getByText("0 of 4 connected")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^Connect$/ }).length).toBeGreaterThanOrEqual(4);
  });
});
