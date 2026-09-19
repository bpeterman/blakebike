import { describe, expect, it } from "vitest";
import {
  connectAllSummary,
  deviceFitsRole,
  deviceName,
  knownToDeviceInfo,
  formatAge,
  formatRelativeDate,
  formatUptime,
  makeAndModel,
  isConnected,
  metricsFedBy,
  readoutMark,
  sourceNote,
  deviceTransport,
  transportLabel,
} from "./devices";
import type { DeviceInfo, DeviceState, KnownConnectOutcome } from "./types";

const device = (capabilities?: DeviceInfo["capabilities"]): DeviceInfo => ({
  id: "x",
  name: "X",
  simulated: false,
  rssi: -60,
  capabilities,
});

describe("device roles", () => {
  it("defaults old device records to BLE and labels ANT devices", () => {
    expect(deviceTransport(device(["heartRate"]))).toBe("ble");
    expect(transportLabel(device(["heartRate"]))).toBe("BLE");
    expect(transportLabel({ ...device(["heartRate"]), transport: "ant" })).toBe("ANT+");
  });

  it("matches devices to roles by advertised service", () => {
    expect(deviceFitsRole(device(["ftms"]), "trainer")).toBe(true);
    expect(deviceFitsRole(device(["ftms"]), "heartRate")).toBe(false);
    expect(deviceFitsRole(device(["heartRate"]), "heartRate")).toBe(true);
    expect(deviceFitsRole(device(["cyclingPower"]), "power")).toBe(true);
    expect(deviceFitsRole(device(["cyclingPower"]), "cadence")).toBe(true);
    expect(deviceFitsRole(device(["csc"]), "cadence")).toBe(true);
    expect(deviceFitsRole(device(["csc"]), "power")).toBe(false);
  });

  it("treats capability-less results as trainers only", () => {
    expect(deviceFitsRole(device(), "trainer")).toBe(true);
    expect(deviceFitsRole(device([]), "heartRate")).toBe(false);
  });

  it("reads connection state", () => {
    const ready: DeviceState = { status: "ready", device: device(["ftms"]) };
    expect(isConnected(ready)).toBe(true);
    expect(isConnected({ status: "connecting", name: "K" })).toBe(false);
    expect(isConnected(undefined)).toBe(false);
    expect(deviceName(ready)).toBe("X");
    expect(deviceName({ status: "idle" })).toBeNull();
  });
});

describe("telemetry provenance", () => {
  it("labels sources and fallbacks", () => {
    expect(sourceNote({ role: "heartRate", fallback: false })).toBe("via heart rate");
    expect(sourceNote({ role: "trainer", fallback: true })).toBe("fallback: trainer");
    expect(sourceNote(null)).toBeNull();
  });

  it("lists which metrics a role feeds", () => {
    const sources = {
      power: { role: "power" as const, fallback: false },
      cadence: { role: "power" as const, fallback: false },
      heartRate: { role: "heartRate" as const, fallback: false },
    };
    expect(metricsFedBy("power", sources)).toEqual(["power", "cadence"]);
    expect(metricsFedBy("trainer", sources)).toEqual([]);
    expect(metricsFedBy("trainer", undefined)).toEqual([]);
  });
});

describe("formatting", () => {
  it("formats uptime and ages", () => {
    expect(formatUptime(4_000)).toBe("4s");
    expect(formatUptime(125_000)).toBe("2m 05s");
    expect(formatUptime(3_725_000)).toBe("1h 02m");
    expect(formatAge(400)).toBe("now");
    expect(formatAge(7_500)).toBe("7s ago");
    expect(formatAge(130_000)).toBe("2m ago");
  });

  it("joins make and model without repeating the brand", () => {
    expect(makeAndModel("Wahoo", "KICKR CORE")).toBe("Wahoo · KICKR CORE");
    expect(makeAndModel("Wahoo Fitness", "Wahoo Fitness KICKR")).toBe("Wahoo Fitness KICKR");
    expect(makeAndModel("Garmin", null)).toBe("Garmin");
    expect(makeAndModel(null, "  ")).toBeNull();
    expect(makeAndModel(null, null)).toBeNull();
  });

  it("formats last-used dates relative to now", () => {
    const now = Date.parse("2026-09-18T12:00:00Z");
    expect(formatRelativeDate("2026-09-18T11:59:40Z", now)).toBe("just now");
    expect(formatRelativeDate("2026-09-18T11:35:00Z", now)).toBe("25 min ago");
    expect(formatRelativeDate("2026-09-18T07:00:00Z", now)).toBe("5 h ago");
    expect(formatRelativeDate("2026-09-17T09:00:00Z", now)).toBe("yesterday");
    expect(formatRelativeDate("2026-09-10T09:00:00Z", now)).toBe("8 days ago");
    expect(formatRelativeDate("garbage", now)).toBe("unknown");
  });

  it("marks log levels", () => {
    expect(readoutMark("ok")).toBe("✓");
    expect(readoutMark("warn")).toBe("!");
    expect(readoutMark("error")).toBe("✕");
    expect(readoutMark("info")).toBe("›");
  });
});

describe("connect all", () => {
  const outcome = (role: KnownConnectOutcome["role"], name: string, status: KnownConnectOutcome["status"]): KnownConnectOutcome =>
    ({ role, name, status, error: status === "failed" ? "Device was not seen within 10s" : null });

  it("turns a remembered device into a connect request without a stale signal reading", () => {
    expect(
      knownToDeviceInfo({
        id: "strap",
        name: "HRM-Pro",
        transport: "ant",
        role: "heartRate",
        capabilities: ["heartRate"],
        simulated: false,
        manufacturer: "Garmin",
        model: null,
        lastConnectedAt: "2026-09-01T00:00:00Z",
      }),
    ).toEqual({ id: "strap", name: "HRM-Pro", transport: "ant", simulated: false, rssi: null, capabilities: ["heartRate"] });
  });

  it("stays quiet when every device answered", () => {
    expect(connectAllSummary([])).toBeNull();
    expect(connectAllSummary([outcome("trainer", "KICKR", "connected"), outcome("heartRate", "HRM", "skipped")])).toBeNull();
  });

  it("names a sleeping sensor gently, without the raw error", () => {
    const summary = connectAllSummary([
      outcome("trainer", "KICKR CORE", "connected"),
      outcome("heartRate", "HRM-Pro", "connected"),
      outcome("power", "Assioma", "failed"),
    ]);
    expect(summary).toBe(
      "Trainer and heart rate connected. Power meter (Assioma) didn’t answer, probably asleep. Wake it and press Connect on its card.",
    );
    expect(summary).not.toContain("10s");
  });

  it("handles nothing connecting and several sleepers", () => {
    expect(connectAllSummary([outcome("power", "Assioma", "failed"), outcome("cadence", "Wahoo RPM", "failed")])).toBe(
      "Power meter (Assioma) and cadence sensor (Wahoo RPM) didn’t answer, probably asleep. Wake them and press Connect on their card.",
    );
  });
});
