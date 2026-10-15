import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { playBeep } from "./beep";
import { LapTimerCard } from "./LapTimerCard";
import { defaultLapTimerSettings, type LapTimerSettings } from "./lapTimer";
import type { LapMark, Telemetry } from "./types";

vi.mock("./beep", () => ({ playBeep: vi.fn(), primeBeep: vi.fn() }));

const history: Telemetry[] = Array.from({ length: 200 }, (_, second) => ({
  timestampMs: second * 1_000,
  powerWatts: 200,
  cadenceRpm: 95,
  speedKph: 30,
  heartRateBpm: 140,
  targetPowerWatts: null,
}));

const drill: LapTimerSettings = { mode: "countdown", workSeconds: 30, restSeconds: 30 };

const card = (timer: LapTimerSettings, elapsedSeconds: number, laps: LapMark[] = [], onLap = vi.fn()) => (
  <LapTimerCard
    timer={timer}
    laps={laps}
    elapsedSeconds={elapsedSeconds}
    history={history.slice(0, elapsedSeconds + 1)}
    onLap={onLap}
  />
);

beforeEach(() => vi.mocked(playBeep).mockClear());
afterEach(cleanup);

describe("lap timer card", () => {
  it("shows the lap being ridden, the last one, and their averages", () => {
    const onLap = vi.fn();
    render(
      card(
        defaultLapTimerSettings,
        150,
        [
          { elapsedSeconds: 60, timestampMs: 60_000 },
          { elapsedSeconds: 125, timestampMs: 125_000 },
        ],
        onLap,
      ),
    );
    expect(screen.getByLabelText("Lap time")).toHaveTextContent("0:25");
    expect(screen.getByText("Lap 3")).toBeInTheDocument();
    expect(screen.getByText("1:05")).toBeInTheDocument();
    expect(screen.getAllByText("95 rpm")).toHaveLength(2);
    expect(screen.getByText("2 laps · press L")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Lap" }));
    expect(onLap).toHaveBeenCalledOnce();
  });

  it("counts a drill down and beeps and flashes at each rollover", () => {
    const { rerender } = render(card(drill, 28));
    expect(screen.getByLabelText("Drill time left")).toHaveTextContent("0:02");
    expect(screen.getByText("On · rep 1 · lap 1")).toBeInTheDocument();
    rerender(card(drill, 29));
    expect(playBeep).not.toHaveBeenCalled();
    rerender(card(drill, 30));
    expect(screen.getByText("Off · rep 1 · lap 1")).toBeInTheDocument();
    expect(playBeep).toHaveBeenCalledOnce();
    expect(screen.getByTestId("lap-timer")).toHaveClass("flash", "resting");
    // A paused ride re-renders without the clock moving: no second cue.
    rerender(card(drill, 30));
    expect(playBeep).toHaveBeenCalledOnce();
  });

  it("restarts the drill quietly when the rider marks a lap", () => {
    const { rerender } = render(card(drill, 45));
    rerender(card(drill, 46, [{ elapsedSeconds: 46, timestampMs: 46_000 }]));
    expect(screen.getByLabelText("Drill time left")).toHaveTextContent("0:30");
    expect(screen.getByText("On · rep 1 · lap 2")).toBeInTheDocument();
    expect(playBeep).not.toHaveBeenCalled();
  });
});
