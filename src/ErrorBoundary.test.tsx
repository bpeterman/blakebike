import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "./api";
import { ErrorBoundary } from "./ErrorBoundary";

vi.mock("./api", () => ({
  api: {
    reportError: vi.fn(async () => undefined),
    pauseOrResume: vi.fn(async () => undefined),
    stopWorkout: vi.fn(async () => undefined),
  },
}));

function Boom(): never {
  throw new Error("chart exploded");
}

describe("ErrorBoundary", () => {
  let consoleError: ReturnType<typeof vi.spyOn>;
  beforeEach(() => {
    // React logs the caught error; keep the test output clean.
    consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    vi.mocked(api.stopWorkout).mockClear();
    vi.mocked(api.pauseOrResume).mockClear();
  });
  afterEach(() => {
    cleanup();
    consoleError.mockRestore();
  });

  it("renders children when nothing goes wrong", () => {
    render(<ErrorBoundary><p>all good</p></ErrorBoundary>);
    expect(screen.getByText("all good")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("keeps ride controls available when the screen crashes", () => {
    render(<ErrorBoundary><Boom /></ErrorBoundary>);
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("chart exploded");
    expect(api.reportError).toHaveBeenCalledWith("render", expect.stringContaining("chart exploded"));
    fireEvent.click(screen.getByRole("button", { name: "End ride" }));
    expect(api.stopWorkout).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Pause / Resume" }));
    expect(api.pauseOrResume).toHaveBeenCalledTimes(1);
  });
});
