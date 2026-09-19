import type { RunnerState } from "./types";

type SavedPostRideState = Extract<RunnerState, { status: "finished" }>;

export function shouldPromptForPostRide(
  previous: RunnerState,
  next: RunnerState,
): next is SavedPostRideState {
  const rideWasActive =
    previous.status === "running" || previous.status === "paused";
  const rideWasSaved = next.status === "finished";
  return rideWasActive && rideWasSaved;
}
