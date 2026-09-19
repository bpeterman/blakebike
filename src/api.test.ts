import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  open: vi.fn(),
  save: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: mocks.open,
  save: mocks.save,
}));

import { api } from "./api";

beforeEach(() => {
  vi.clearAllMocks();
});

describe("workout library export", () => {
  it("does nothing when folder selection is cancelled", async () => {
    mocks.open.mockResolvedValue(null);

    await expect(api.exportAllZwo()).resolves.toBeNull();
    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it("exports all workouts to the selected folder", async () => {
    mocks.open.mockResolvedValue("/tmp/workout-backup");
    mocks.invoke.mockResolvedValue({
      exportedCount: 3,
      directory: "/tmp/workout-backup",
    });

    await expect(api.exportAllZwo()).resolves.toEqual({
      exportedCount: 3,
      directory: "/tmp/workout-backup",
    });
    expect(mocks.invoke).toHaveBeenCalledWith("export_all_zwo_workouts", {
      directory: "/tmp/workout-backup",
    });
  });

  it("propagates backend export errors", async () => {
    mocks.open.mockResolvedValue("/tmp/workout-backup");
    mocks.invoke.mockRejectedValue(new Error("disk full"));

    await expect(api.exportAllZwo()).rejects.toThrow("disk full");
  });
});
