import { describe, expect, it } from "vitest";
import {
  deviceFitsRole,
  deviceName,
  formatAge,
  formatUptime,
  isConnected,
  metricsFedBy,
  readoutMark,
  sourceNote,
} from "./devices";
import type { DeviceInfo, DeviceState } from "./types";

const device = (capabilities?: DeviceInfo["capabilities"]): DeviceInfo => ({
  id: "x",
  name: "X",
  simulated: false,
  rssi: -60,
  capabilities,
});

describe("device roles", () => {
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

  it("marks log levels", () => {
    expect(readoutMark("ok")).toBe("✓");
    expect(readoutMark("warn")).toBe("!");
    expect(readoutMark("error")).toBe("✕");
    expect(readoutMark("info")).toBe("›");
  });
});
