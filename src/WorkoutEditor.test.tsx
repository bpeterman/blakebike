import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { WorkoutEditor } from "./WorkoutEditor";
import { derivedPowerZones, type Workout, type WorkoutStep } from "./types";

const FTP = 200;

const steady = (durationSeconds: number, value: number): WorkoutStep => ({
  kind: "steady",
  durationSeconds,
  target: { unit: "percentFtp", value },
});

const workout: Workout = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "Tempo",
  description: "Two steady blocks",
  source: "local",
  version: 1,
  createdAt: "2026-01-01T00:00:00Z",
  updatedAt: "2026-01-01T00:00:00Z",
  steps: [steady(300, 75), steady(600, 85)],
};

const renderEditor = (initial: Workout = workout) => {
  const close = vi.fn();
  const save = vi.fn();
  const utils = render(
    <WorkoutEditor
      initial={initial}
      ftpWatts={FTP}
      powerZones={derivedPowerZones(FTP)}
      close={close}
      save={save}
    />,
  );
  return { ...utils, close, save };
};

const blocks = (container: HTMLElement) =>
  [...container.querySelectorAll<SVGPolygonElement>("polygon[data-path]")];

const blockWidth = (block: SVGPolygonElement) => {
  const [[left], [right]] = block
    .getAttribute("points")!
    .split(" ")
    .map((pair) => pair.split(",").map(Number));
  return right - left;
};

const rows = (container: HTMLElement) =>
  [...container.querySelectorAll<HTMLDivElement>(".step-editor")];

afterEach(cleanup);

describe("WorkoutEditor", () => {
  it("previews one block per step and grows as blocks are added", () => {
    const { container } = renderEditor();
    expect(blocks(container)).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: "Ramp" }));
    fireEvent.click(screen.getByRole("button", { name: "Free ride" }));
    const drawn = blocks(container);
    expect(drawn).toHaveLength(4);
    expect(drawn.map((block) => block.dataset.kind)).toEqual(["steady", "steady", "ramp", "freeRide"]);
    expect(screen.getByRole("img", { name: /25:00 workout, 4 blocks/ })).toBeInTheDocument();
  });

  it("resizes a block as soon as its duration changes", () => {
    const { container } = renderEditor();
    const [first, second] = blocks(container);
    expect(blockWidth(first)).toBeLessThan(blockWidth(second));
    const [duration] = screen.getAllByLabelText("Duration (sec)");
    fireEvent.change(duration, { target: { value: "1800" } });
    const [after, afterSecond] = blocks(container);
    expect(blockWidth(after)).toBeGreaterThan(blockWidth(afterSecond));
    expect(blockWidth(after) / (blockWidth(after) + blockWidth(afterSecond))).toBeCloseTo(0.75, 1);
  });

  it("keeps the remaining row's identity and values when the first row is removed", () => {
    const { container } = renderEditor();
    const secondId = rows(container)[1].dataset.rowId;
    fireEvent.click(screen.getByRole("button", { name: "Remove block 1" }));
    const remaining = rows(container);
    expect(remaining).toHaveLength(1);
    expect(remaining[0].dataset.rowId).toBe(secondId);
    expect(screen.getByLabelText("Duration (sec)")).toHaveValue(600);
    expect(screen.getByLabelText("Power (% FTP)")).toHaveValue(85);
    expect(blocks(container)).toHaveLength(1);
  });

  it("links rows and blocks in both directions", () => {
    const { container } = renderEditor();
    fireEvent.mouseEnter(rows(container)[1]);
    expect(blocks(container).map((block) => block.dataset.highlighted)).toEqual([undefined, "true"]);
    expect(rows(container)[1]).toHaveAttribute("data-highlighted", "true");
    fireEvent.mouseLeave(rows(container)[1]);
    expect(blocks(container).map((block) => block.dataset.highlighted)).toEqual([undefined, undefined]);

    fireEvent.mouseEnter(blocks(container)[0]);
    expect(rows(container)[0]).toHaveAttribute("data-highlighted", "true");
    expect(rows(container)[1]).not.toHaveAttribute("data-highlighted");
    fireEvent.mouseLeave(container.querySelector(".workout-profile svg")!);
    expect(rows(container)[0]).not.toHaveAttribute("data-highlighted");
  });

  it("focuses a row's first input when its block is clicked", () => {
    const { container } = renderEditor();
    fireEvent.click(blocks(container)[1]);
    const [, secondDuration] = screen.getAllByLabelText("Duration (sec)");
    expect(document.activeElement).toBe(secondDuration);
    expect(rows(container)[1]).toHaveAttribute("data-highlighted", "true");
  });

  it("summarises duration, intensity, and stress, and calls out free ride", () => {
    const { container } = renderEditor({ ...workout, steps: [steady(3600, 100)] });
    const stats = () => within(container.querySelector<HTMLElement>(".editor-stats")!);
    expect(stats().getByText("1:00:00")).toBeInTheDocument();
    expect(stats().getByText("100%")).toBeInTheDocument();
    expect(stats().getByText("100")).toBeInTheDocument();
    expect(stats().getByText("1")).toBeInTheDocument();
    expect(stats().queryByText(/free ride is not counted/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Free ride" }));
    expect(stats().getByText("1:05:00")).toBeInTheDocument();
    expect(stats().getByText("5:00 of free ride is not counted in intensity")).toBeInTheDocument();
    expect(stats().getByText("100%")).toBeInTheDocument();
    expect(stats().getByText("2")).toBeInTheDocument();
  });

  it("saves a plain workout with edited steps and no editor-only fields", () => {
    const { save } = renderEditor();
    fireEvent.change(screen.getByLabelText("Workout name"), { target: { value: "Tempo+" } });
    const [, secondPower] = screen.getAllByLabelText("Power (% FTP)");
    fireEvent.change(secondPower, { target: { value: "90" } });
    fireEvent.click(screen.getByRole("button", { name: "Save workout" }));
    expect(save).toHaveBeenCalledOnce();
    expect(save.mock.calls[0][0]).toEqual({
      ...workout,
      name: "Tempo+",
      steps: [steady(300, 75), steady(600, 90)],
    });
  });

  it("keeps watt targets in watts when they are edited", () => {
    const { save } = renderEditor({
      ...workout,
      steps: [{ kind: "steady", durationSeconds: 120, target: { unit: "watts", value: 250 } }],
    });
    const power = screen.getByLabelText("Power (W)");
    expect(power).toHaveValue(250);
    fireEvent.change(power, { target: { value: "260" } });
    fireEvent.click(screen.getByRole("button", { name: "Save workout" }));
    expect(save.mock.calls[0][0].steps).toEqual([
      { kind: "steady", durationSeconds: 120, target: { unit: "watts", value: 260 } },
    ]);
  });

  it("only allows saving a named workout with at least one block", () => {
    const { container } = renderEditor({ ...workout, steps: [] });
    expect(screen.getByRole("button", { name: "Save workout" })).toBeDisabled();
    expect(screen.getByText("Add a block to see the shape of your workout")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Steady" }));
    expect(screen.getByRole("button", { name: "Save workout" })).toBeEnabled();
    expect(blocks(container)).toHaveLength(1);
    fireEvent.change(screen.getByLabelText("Workout name"), { target: { value: "   " } });
    expect(screen.getByRole("button", { name: "Save workout" })).toBeDisabled();
  });
});
