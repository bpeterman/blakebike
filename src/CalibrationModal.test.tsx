import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CalibrationProgress, CalibrationRecord, DeviceSlot } from "./types";

let progressHandler: ((progress: CalibrationProgress) => void) | undefined;
const unlisten = vi.fn();

vi.mock("./api", () => ({
  api: {
    onCalibrationProgress: vi.fn((handler: (progress: CalibrationProgress) => void) => {
      progressHandler = handler;
      return Promise.resolve(unlisten);
    }),
    calibrateDevice: vi.fn(() => Promise.resolve({} as CalibrationRecord)),
    cancelCalibration: vi.fn(() => Promise.resolve()),
    disconnectDevice: vi.fn(() => Promise.resolve()),
    reportError: vi.fn(() => Promise.resolve()),
  },
}));

import { api } from "./api";
import { CalibrationModal } from "./CalibrationModal";

const record: CalibrationRecord = { at: new Date().toISOString(), kind: "zeroOffset", offsetRaw: 1019 };

const powerSlot = (lastCalibration: CalibrationRecord | null = null): DeviceSlot => ({
  role: "power",
  state: { status: "ready", device: { id: "pm", name: "Assioma", simulated: false, rssi: -50, capabilities: ["cyclingPower"] } },
  stats: {
    samples: 10,
    parseFailures: 0,
    lastSampleMs: Date.now(),
    rateHz: 1,
    rssi: -50,
    batteryPercent: 80,
    batteryStatus: null,
    batteryVoltage: null,
    manufacturer: "Favero",
    model: null,
    firmware: null,
    connectedSinceMs: Date.now() - 10_000,
    drops: 0,
    lastRawHex: null,
    lastReading: "0 W · 0 rpm",
    calibrationSupported: true,
    calibrating: false,
    calibrationRequested: false,
    lastCalibration,
  },
  log: [],
});

describe("CalibrationModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    progressHandler = undefined;
    vi.mocked(api.calibrateDevice).mockResolvedValue(record);
  });

  afterEach(cleanup);

  describe("trainer spin-down", () => {
    it("starts calibration and follows live speed and phase events", async () => {
      const close = vi.fn();
      const view = render(<CalibrationModal role="trainer" slot={undefined} speedKph={31.4} close={close} />);
      await waitFor(() => expect(progressHandler).toBeDefined());

      fireEvent.click(screen.getByRole("button", { name: "Begin calibration" }));
      expect(api.calibrateDevice).toHaveBeenCalledWith("trainer");
      expect(screen.getByRole("button", { name: "Close calibration" })).toBeDisabled();

      act(() => progressHandler?.({
        role: "trainer",
        phase: "accelerate",
        detail: { kind: "spinDown", targetLowKph: 30, targetHighKph: 35 },
        message: "Pedal into range.",
      }));
      expect(screen.getByText(/31\.4/)).toBeInTheDocument();
      expect(screen.getByText("Target 30.0–35.0 km/h")).toBeInTheDocument();

      // A power-meter event meanwhile is somebody else's.
      act(() => progressHandler?.({ role: "power", phase: "error", detail: null, message: "Not ours." }));
      expect(screen.queryByText("Not ours.")).not.toBeInTheDocument();

      act(() => progressHandler?.({
        role: "trainer",
        phase: "stopPedaling",
        detail: { kind: "spinDown", targetLowKph: 30, targetHighKph: 35 },
        message: "Stop now.",
      }));
      expect(screen.getByRole("heading", { name: /Stop pedaling/ })).toBeInTheDocument();

      act(() => progressHandler?.({ role: "trainer", phase: "success", detail: null, message: "Done." }));
      fireEvent.click(screen.getByRole("button", { name: "Close" }));
      expect(close).toHaveBeenCalledOnce();

      view.unmount();
      expect(unlisten).toHaveBeenCalledOnce();
    });

    it("shows immediate command failures and allows retry", async () => {
      vi.mocked(api.calibrateDevice).mockRejectedValueOnce(new Error("Trainer disconnected"));
      render(<CalibrationModal role="trainer" slot={undefined} speedKph={null} close={vi.fn()} />);
      await waitFor(() => expect(progressHandler).toBeDefined());

      fireEvent.click(screen.getByRole("button", { name: "Begin calibration" }));
      await screen.findByText("Trainer disconnected");
      expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
      expect(api.reportError).toHaveBeenCalledWith("trainer calibration", "Trainer disconnected");
    });

    it("cancels an active calibration by disconnecting the trainer", async () => {
      const close = vi.fn();
      render(<CalibrationModal role="trainer" slot={undefined} speedKph={20} close={close} />);
      await waitFor(() => expect(progressHandler).toBeDefined());
      fireEvent.click(screen.getByRole("button", { name: "Begin calibration" }));
      fireEvent.click(screen.getByRole("button", { name: "Cancel and disconnect" }));
      await waitFor(() => expect(api.disconnectDevice).toHaveBeenCalledWith("trainer"));
      expect(api.cancelCalibration).not.toHaveBeenCalled();
      expect(close).toHaveBeenCalledOnce();
    });
  });

  describe("power meter zero offset", () => {
    it("explains the procedure, shows the meter reading while it runs, then the offset and its drift", async () => {
      render(<CalibrationModal role="power" slot={powerSlot(record)} speedKph={null} close={vi.fn()} />);
      await waitFor(() => expect(progressHandler).toBeDefined());
      expect(screen.getByRole("heading", { name: "Zero the power meter" })).toBeInTheDocument();
      expect(screen.getByText(/Unclip and stand the bike upright/)).toBeInTheDocument();
      expect(screen.getByText(/Last zero on record: offset 1019/)).toBeInTheDocument();

      fireEvent.click(screen.getByRole("button", { name: "Begin zero offset" }));
      expect(api.calibrateDevice).toHaveBeenCalledWith("power");
      act(() => progressHandler?.({ role: "power", phase: "holdStill", detail: null, message: "Keep the bike still." }));
      expect(screen.getByRole("heading", { name: "Hold still" })).toBeInTheDocument();
      expect(screen.getByText("0 W · 0 rpm")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument();

      act(() => progressHandler?.({
        role: "power",
        phase: "success",
        detail: { kind: "zeroOffset", offsetRaw: 1023, previousOffsetRaw: 1019 },
        message: "Zero offset complete.",
      }));
      expect(screen.getByRole("heading", { name: "Zero offset complete" })).toBeInTheDocument();
      expect(screen.getByText("1023")).toBeInTheDocument();
      expect(screen.getByText(/was 1019 · drift \+4 · steady/)).toBeInTheDocument();
      expect(screen.queryByText(/big jump/)).not.toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Zero again" })).toBeInTheDocument();
    });

    it("flags a large drift and a first zero", async () => {
      const view = render(<CalibrationModal role="power" slot={powerSlot()} speedKph={null} close={vi.fn()} />);
      await waitFor(() => expect(progressHandler).toBeDefined());
      fireEvent.click(screen.getByRole("button", { name: "Begin zero offset" }));
      act(() => progressHandler?.({
        role: "power",
        phase: "success",
        detail: { kind: "zeroOffset", offsetRaw: 1100, previousOffsetRaw: 1000 },
        message: "Zero offset complete.",
      }));
      expect(screen.getByText(/drift \+100 · large change/)).toBeInTheDocument();
      expect(screen.getByText(/big jump from last time/)).toBeInTheDocument();
      view.unmount();

      render(<CalibrationModal role="power" slot={powerSlot()} speedKph={null} close={vi.fn()} />);
      await waitFor(() => expect(progressHandler).toBeDefined());
      fireEvent.click(screen.getByRole("button", { name: "Begin zero offset" }));
      act(() => progressHandler?.({
        role: "power",
        phase: "success",
        detail: { kind: "zeroOffset", offsetRaw: 512, previousOffsetRaw: null },
        message: "Zero offset complete.",
      }));
      expect(screen.getByText("first zero on record")).toBeInTheDocument();
    });

    it("cancels by asking the hub to stop waiting, without disconnecting", async () => {
      const close = vi.fn();
      render(<CalibrationModal role="power" slot={powerSlot()} speedKph={null} close={close} />);
      await waitFor(() => expect(progressHandler).toBeDefined());
      fireEvent.click(screen.getByRole("button", { name: "Begin zero offset" }));
      fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
      await waitFor(() => expect(api.cancelCalibration).toHaveBeenCalledWith("power"));
      expect(api.disconnectDevice).not.toHaveBeenCalled();
      expect(close).toHaveBeenCalledOnce();
    });

    it("shows a refusal and offers to try again", async () => {
      vi.mocked(api.calibrateDevice).mockRejectedValueOnce(new Error("The meter is still moving (84 rpm). Unclip, stop the cranks and try again."));
      render(<CalibrationModal role="power" slot={powerSlot()} speedKph={null} close={vi.fn()} />);
      await waitFor(() => expect(progressHandler).toBeDefined());
      fireEvent.click(screen.getByRole("button", { name: "Begin zero offset" }));
      await screen.findByText(/still moving \(84 rpm\)/);
      expect(screen.getByRole("heading", { name: "Zero offset did not complete" })).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
      expect(api.reportError).toHaveBeenCalledWith("power calibration", expect.stringContaining("still moving"));
    });
  });
});
