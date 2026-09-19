import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CalibrationProgress } from "./types";

let progressHandler: ((progress: CalibrationProgress) => void) | undefined;
const unlisten = vi.fn();

vi.mock("./api", () => ({
  api: {
    onCalibrationProgress: vi.fn((handler: (progress: CalibrationProgress) => void) => {
      progressHandler = handler;
      return Promise.resolve(unlisten);
    }),
    calibrateTrainer: vi.fn(() => Promise.resolve()),
    disconnectDevice: vi.fn(() => Promise.resolve()),
    reportError: vi.fn(() => Promise.resolve()),
  },
}));

import { api } from "./api";
import { TrainerCalibrationModal } from "./TrainerCalibrationModal";

describe("TrainerCalibrationModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    progressHandler = undefined;
    vi.mocked(api.calibrateTrainer).mockResolvedValue();
  });

  afterEach(cleanup);

  it("starts calibration and follows live speed and phase events", async () => {
    const close = vi.fn();
    const view = render(<TrainerCalibrationModal speedKph={31.4} close={close} />);
    await waitFor(() => expect(progressHandler).toBeDefined());

    fireEvent.click(screen.getByRole("button", { name: "Begin calibration" }));
    expect(api.calibrateTrainer).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: "Close calibration" })).toBeDisabled();

    act(() => progressHandler?.({
      phase: "accelerate",
      targetLowKph: 30,
      targetHighKph: 35,
      message: "Pedal into range.",
    }));
    expect(screen.getByText(/31\.4/)).toBeInTheDocument();
    expect(screen.getByText("Target 30.0–35.0 km/h")).toBeInTheDocument();

    act(() => progressHandler?.({
      phase: "stopPedaling",
      targetLowKph: 30,
      targetHighKph: 35,
      message: "Stop now.",
    }));
    expect(screen.getByRole("heading", { name: /Stop pedaling/ })).toBeInTheDocument();

    act(() => progressHandler?.({
      phase: "success",
      targetLowKph: null,
      targetHighKph: null,
      message: "Done.",
    }));
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(close).toHaveBeenCalledOnce();

    view.unmount();
    expect(unlisten).toHaveBeenCalledOnce();
  });

  it("shows immediate command failures and allows retry", async () => {
    vi.mocked(api.calibrateTrainer).mockRejectedValueOnce(new Error("Trainer disconnected"));
    render(<TrainerCalibrationModal speedKph={null} close={vi.fn()} />);
    await waitFor(() => expect(progressHandler).toBeDefined());

    fireEvent.click(screen.getByRole("button", { name: "Begin calibration" }));
    await screen.findByText("Trainer disconnected");
    expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
    expect(api.reportError).toHaveBeenCalledWith("trainer calibration", "Trainer disconnected");
  });

  it("cancels an active calibration by disconnecting the trainer", async () => {
    const close = vi.fn();
    render(<TrainerCalibrationModal speedKph={20} close={close} />);
    await waitFor(() => expect(progressHandler).toBeDefined());
    fireEvent.click(screen.getByRole("button", { name: "Begin calibration" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel and disconnect" }));
    await waitFor(() => expect(api.disconnectDevice).toHaveBeenCalledWith("trainer"));
    expect(close).toHaveBeenCalledOnce();
  });
});
