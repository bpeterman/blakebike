import type { RunnerState } from "./types";

type SavedPostRideState =
  | Extract<RunnerState, { status: "finished" }>
  | (Extract<RunnerState, { status: "error" }> & { sessionId: string });

export function shouldPromptForPostRide(
  previous: RunnerState,
  next: RunnerState,
): next is SavedPostRideState {
  const rideWasActive =
    previous.status === "running" || previous.status === "paused";
  const rideWasSaved =
    next.status === "finished" ||
    (next.status === "error" && next.sessionId !== null);
  return rideWasActive && rideWasSaved;
}
