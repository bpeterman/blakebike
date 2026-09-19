import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { WorkoutLibrary } from "./App";
import type { Workout } from "./types";

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
