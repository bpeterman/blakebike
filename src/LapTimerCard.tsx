import { useEffect, useRef, useState } from "react";
import { Flag } from "lucide-react";
import { playBeep, primeBeep } from "./beep";
import { drillPhase, lapStatus, type LapAverages, type LapTimerSettings } from "./lapTimer";
import { formatDuration, type LapMark, type Telemetry } from "./types";

/** How long the card lights up at a drill rollover. */
const FLASH_MS = 700;
/** A higher note into work, a lower one into rest. */
const WORK_BEEP_HZ = 1_046;
const REST_BEEP_HZ = 659;

const whole = (value: number | null, unit: string): string =>
  value === null ? "—" : `${Math.round(value)} ${unit}`;

function LapColumn({
  heading,
  seconds,
  averages,
}: {
  heading: string;
  seconds: number | null;
  averages: LapAverages | null;
}) {
  return (
    <div className="lap-timer-column">
      <span className="label">{heading}</span>
      <strong>{seconds === null ? "—" : formatDuration(seconds)}</strong>
      <span>{whole(averages?.cadenceRpm ?? null, "rpm")}</span>
      <span>{whole(averages?.powerWatts ?? null, "W")}</span>
    </div>
  );
}

/**
 * The lap timer on a ride screen: the lap being ridden, the one before it and
 * their average cadence and power. As a countdown drill it also counts down
 * each work and rest part, beeping and flashing at every rollover. The clock
 * is the ride clock, so everything holds still while the ride is paused.
 */
export function LapTimerCard({
  timer,
  laps,
  elapsedSeconds,
  history,
  onLap,
}: {
  timer: LapTimerSettings;
  laps: readonly LapMark[];
  elapsedSeconds: number;
  history: readonly Telemetry[];
  onLap: () => void;
}) {
  const status = lapStatus(laps, elapsedSeconds, history);
  const phase = timer.mode === "countdown" ? drillPhase(timer, status.seconds) : null;
  const [flashing, setFlashing] = useState(false);
  // The rollover last cued, so a re-render, a new lap or the card appearing
  // mid-drill does not beep: only moving on to the next part within a lap does.
  const cued = useRef<{ lap: number; index: number } | null>(null);
  const lapNumber = status.number;
  const phaseIndex = phase?.index ?? null;
  const phaseKind = phase?.kind ?? null;

  useEffect(() => {
    if (phaseIndex === null) {
      cued.current = null;
      return;
    }
    const previous = cued.current;
    cued.current = { lap: lapNumber, index: phaseIndex };
    if (previous === null || previous.lap !== lapNumber || phaseIndex <= previous.index) return;
    playBeep(phaseKind === "rest" ? REST_BEEP_HZ : WORK_BEEP_HZ);
    setFlashing(true);
    const timeout = window.setTimeout(() => setFlashing(false), FLASH_MS);
    return () => window.clearTimeout(timeout);
  }, [lapNumber, phaseIndex, phaseKind]);

  const className = [
    "card",
    "lap-timer-card",
    phase?.kind === "rest" ? "resting" : "",
    flashing ? "flash" : "",
  ]
    .filter((name) => name !== "")
    .join(" ");

  return (
    <div className={className} data-testid="lap-timer">
      <div className="lap-timer-main">
        <span className="label">
          <Flag size={14} /> {phase === null ? "LAP TIMER" : "COUNTDOWN DRILL"}
        </span>
        <strong className="lap-timer-clock" aria-label={phase === null ? "Lap time" : "Drill time left"}>
          {formatDuration(phase === null ? status.seconds : phase.remainingSeconds)}
        </strong>
        <span className="lap-timer-caption">
          {phase === null
            ? `Lap ${status.number}`
            : `${phase.kind === "work" ? "On" : "Off"} · rep ${phase.repeat} · lap ${status.number}`}
        </span>
      </div>
      <LapColumn heading="THIS LAP" seconds={status.seconds} averages={status.current} />
      <LapColumn heading="LAST LAP" seconds={status.lastSeconds} averages={status.last} />
      <div className="lap-timer-action">
        <button
          type="button"
          className="primary"
          onClick={() => {
            primeBeep();
            onLap();
          }}
        >
          <Flag size={16} /> Lap
        </button>
        <span>
          {status.number - 1} {status.number === 2 ? "lap" : "laps"} · press L
        </span>
      </div>
    </div>
  );
}
