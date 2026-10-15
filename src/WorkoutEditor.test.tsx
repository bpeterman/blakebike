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

  describe("repeat blocks", () => {
    const intervals: Workout = {
      ...workout,
      steps: [
        steady(600, 55),
        { kind: "repeat", repetitions: 3, steps: [steady(120, 110), steady(60, 50)] },
        steady(300, 50),
      ],
    };
    const group = (container: HTMLElement) => container.querySelector<HTMLElement>(".repeat-group")!;
    const childRows = (container: HTMLElement) =>
      [...group(container).querySelectorAll<HTMLDivElement>(".repeat-children .step-editor")];
    const save = () => fireEvent.click(screen.getByRole("button", { name: "Save workout" }));

    it("edits an existing repeat's count and children and previews every iteration", () => {
      const { container, save: onSave } = renderEditor(intervals);
      expect(blocks(container)).toHaveLength(1 + 3 * 2 + 1);
      expect(within(group(container)).getByText("3 × 3:00 = 9:00")).toBeInTheDocument();

      fireEvent.change(screen.getByLabelText("Repeats"), { target: { value: "5" } });
      expect(blocks(container)).toHaveLength(1 + 5 * 2 + 1);
      expect(container.querySelector("[data-repeat='1'] text")).toHaveTextContent("5×");

      const [work] = childRows(container);
      fireEvent.change(within(work).getByLabelText("Duration (sec)"), { target: { value: "90" } });
      fireEvent.click(screen.getByRole("button", { name: "Add ramp to repeat 2" }));
      expect(blocks(container)).toHaveLength(1 + 5 * 3 + 1);
      expect(blocks(container).filter((block) => block.dataset.path === "1.2")).toHaveLength(5);

      save();
      expect(onSave.mock.calls[0][0].steps).toEqual([
        steady(600, 55),
        {
          kind: "repeat",
          repetitions: 5,
          steps: [
            steady(90, 110),
            steady(60, 50),
            {
              kind: "ramp",
              durationSeconds: 300,
              start: { unit: "percentFtp", value: 50 },
              end: { unit: "percentFtp", value: 90 },
            },
          ],
        },
        steady(300, 50),
      ]);
    });

    it("reorders and removes children while keeping each row's identity and draft", () => {
      const { container, save: onSave } = renderEditor(intervals);
      const [workId, restId] = childRows(container).map((row) => row.dataset.rowId);
      fireEvent.click(screen.getByRole("button", { name: "Move block 2.2 up" }));
      expect(childRows(container).map((row) => row.dataset.rowId)).toEqual([restId, workId]);
      expect(within(childRows(container)[0]).getByLabelText("Power (% FTP)")).toHaveValue(50);
      expect(blocks(container)[1].dataset.path).toBe("1.0");

      fireEvent.click(screen.getByRole("button", { name: "Remove block 2.1" }));
      expect(childRows(container).map((row) => row.dataset.rowId)).toEqual([workId]);
      expect(within(childRows(container)[0]).getByLabelText("Power (% FTP)")).toHaveValue(110);

      // Moving the last child down steps it out of the repeat.
      fireEvent.click(screen.getByRole("button", { name: "Move block 2.1 down" }));
      expect(screen.getByText("This repeat is empty. Add a step or remove it.")).toBeInTheDocument();
      expect(screen.getByRole("status")).toHaveTextContent("Repeat 2 needs at least one step");
      expect(screen.getByRole("button", { name: "Save workout" })).toBeDisabled();
      fireEvent.click(screen.getByRole("button", { name: "Remove block 2" }));
      save();
      expect(onSave.mock.calls[0][0].steps).toEqual([steady(600, 55), steady(120, 110), steady(300, 50)]);
    });

    it("adds a work/recovery repeat from the block bar", () => {
      const { container, save: onSave } = renderEditor();
      fireEvent.click(screen.getByRole("button", { name: "Repeat" }));
      expect(blocks(container)).toHaveLength(2 + 4 * 2);
      save();
      expect(onSave.mock.calls[0][0].steps[2]).toEqual({
        kind: "repeat",
        repetitions: 4,
        steps: [steady(60, 110), steady(60, 55)],
      });
    });

    it("turns a step or a selection of neighbouring steps into a repeat, and back", () => {
      const { container, save: onSave } = renderEditor({ ...workout, steps: [steady(300, 75), steady(60, 120), steady(60, 50)] });
      const ids = rows(container).map((row) => row.dataset.rowId);

      fireEvent.click(screen.getByLabelText("Select block 1"));
      fireEvent.click(screen.getByLabelText("Select block 3"));
      expect(screen.getByRole("button", { name: "Repeat 2 selected" })).toBeDisabled();
      fireEvent.click(screen.getByLabelText("Select block 1"));
      fireEvent.click(screen.getByLabelText("Select block 2"));
      fireEvent.click(screen.getByRole("button", { name: "Repeat 2 selected" }));
      expect(screen.queryByRole("button", { name: /selected/ })).not.toBeInTheDocument();
      expect(childRows(container).map((row) => row.dataset.rowId)).toEqual([ids[1], ids[2]]);
      expect(blocks(container)).toHaveLength(1 + 2 * 2);

      fireEvent.click(screen.getByRole("button", { name: "Repeat block 1" }));
      save();
      expect(onSave.mock.calls[0][0].steps).toEqual([
        { kind: "repeat", repetitions: 2, steps: [steady(300, 75)] },
        { kind: "repeat", repetitions: 2, steps: [steady(60, 120), steady(60, 50)] },
      ]);

      fireEvent.click(screen.getByRole("button", { name: "Ungroup block 2" }));
      expect(rows(container).map((row) => row.dataset.rowId).slice(-2)).toEqual([ids[1], ids[2]]);
    });

    it("links nested rows and expanded blocks in both directions", () => {
      const { container } = renderEditor(intervals);
      const highlighted = () =>
        blocks(container).flatMap((block) => (block.dataset.highlighted ? [block.dataset.path] : []));
      fireEvent.mouseEnter(container.querySelector(".repeat-header")!);
      expect(highlighted()).toEqual(["1.0", "1.1", "1.0", "1.1", "1.0", "1.1"]);
      fireEvent.mouseEnter(childRows(container)[1]);
      expect(highlighted()).toEqual(["1.1", "1.1", "1.1"]);

      fireEvent.mouseEnter(blocks(container)[3]);
      expect(childRows(container)[0]).toHaveAttribute("data-highlighted", "true");
      expect(container.querySelector(".repeat-header")).not.toHaveAttribute("data-highlighted");
      fireEvent.click(blocks(container)[4]);
      expect(document.activeElement).toBe(within(childRows(container)[1]).getByLabelText("Duration (sec)"));
    });

    it("explains an out-of-range repeat count instead of saving it", () => {
      renderEditor(intervals);
      fireEvent.change(screen.getByLabelText("Repeats"), { target: { value: "25" } });
      expect(screen.getByRole("status")).toHaveTextContent("Repeat 2 must run between 2 and 20 times");
      expect(screen.getByRole("button", { name: "Save workout" })).toBeDisabled();
    });
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
