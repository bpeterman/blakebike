import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { WorkoutLibrary } from "./App";
import { derivedPowerZones, type Workout } from "./types";

const workout: Workout = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "Tempo",
  description: "A steady ride",
  source: "local",
  version: 1,
  createdAt: "2026-01-01T00:00:00Z",
  updatedAt: "2026-01-01T00:00:00Z",
  steps: [
    {
      kind: "steady",
      durationSeconds: 300,
      target: { unit: "percentFtp", value: 75 },
    },
  ],
};

afterEach(cleanup);

describe("WorkoutLibrary", () => {
  const props = {
    ftp: 200,
    powerZones: derivedPowerZones(200),
    onCreate: vi.fn(),
    onEdit: vi.fn(),
    onRide: vi.fn(),
    onDelete: vi.fn(),
    onExport: vi.fn(),
    onExportAll: vi.fn(),
    onImport: vi.fn(),
  };

  it("exports the complete workout library", () => {
    const onExportAll = vi.fn();
    render(
      <WorkoutLibrary
        {...props}
        workouts={[workout]}
        onExportAll={onExportAll}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Export all ZWO" }));
    expect(onExportAll).toHaveBeenCalledOnce();
  });

  it("shows mirrored Intervals.icu workouts read-only, with their folder and a copy to edit", () => {
    const onEdit = vi.fn();
    const onDelete = vi.fn();
    const mirrored: Workout = {
      ...workout,
      id: "22222222-2222-4222-8222-222222222222",
      name: "Threshold 2x20",
      source: "intervals",
      origin: { externalId: 7, folderId: 3, folder: "Base", updated: "2026-09-01T08:00:00", plannedLoad: 92 },
    };
    render(<WorkoutLibrary {...props} workouts={[mirrored, workout]} onEdit={onEdit} onDelete={onDelete} />);

    expect(screen.getByText("INTERVALS.ICU · BASE · 5 MIN")).toBeInTheDocument();
    expect(screen.getByText("LOCAL · 5 MIN")).toBeInTheDocument();
    expect(screen.getByText("92 load")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete Threshold 2x20" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete Tempo" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Edit a copy" }));
    expect(onEdit).toHaveBeenCalledOnce();
    const copy = onEdit.mock.calls[0][0] as Workout;
    expect(copy.id).not.toBe(mirrored.id);
    expect(copy.source).toBe("local");
    expect(copy.origin).toBeNull();
    expect(copy.name).toBe("Threshold 2x20 (copy)");
    expect(copy.steps).toEqual(mirrored.steps);

    fireEvent.click(screen.getByRole("button", { name: "Edit Tempo" }));
    expect(onEdit).toHaveBeenLastCalledWith(workout);
    expect(onDelete).not.toHaveBeenCalled();
  });

  it("disables bulk export when the library is empty", () => {
    render(<WorkoutLibrary {...props} workouts={[]} />);
    expect(
      screen.getByRole("button", { name: "Export all ZWO" }),
    ).toBeDisabled();
    expect(screen.getByRole("heading", { name: "Your next ride starts here" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Create a workout" }));
    expect(props.onCreate).toHaveBeenCalled();
  });
});
