import { useCallback, useMemo, useRef, useState } from "react";
import { Plus, Trash2, X } from "lucide-react";
import type { PowerTarget, Workout, WorkoutStep, ZoneDefinition } from "./types";
import { formatDuration, workoutDuration } from "./types";
import { EditableNumberInput } from "./EditableNumberInput";
import { Metric } from "./Metric";
import { WorkoutProfile } from "./WorkoutProfile";
import { useDialog } from "./useDialog";
import { profileSegments, profileStats } from "./workoutProfileModel";
import type { LeafStep, StepPath } from "./workoutSteps";

/**
 * A top-level step with an identity that survives reordering and deletion.
 * Ids exist only while the editor is open; `toWorkout` strips them.
 */
type EditorRow = { id: string; step: WorkoutStep };

type EditorState = {
  meta: Omit<Workout, "steps">;
  rows: EditorRow[];
};

let nextRowId = 0;
const newRowId = () => `row-${(nextRowId += 1)}`;

const toEditorState = (workout: Workout): EditorState => {
  const { steps, ...meta } = structuredClone(workout);
  return { meta, rows: steps.map((step) => ({ id: newRowId(), step })) };
};

const toWorkout = (state: EditorState): Workout => ({
  ...state.meta,
  steps: state.rows.map((row) => row.step),
});

const defaultStep = (kind: LeafStep["kind"]): WorkoutStep => {
  switch (kind) {
    case "steady":
      return { kind, durationSeconds: 300, target: { unit: "percentFtp", value: 75 } };
    case "ramp":
      return {
        kind,
        durationSeconds: 300,
        start: { unit: "percentFtp", value: 50 },
        end: { unit: "percentFtp", value: 90 },
      };
    case "freeRide":
      return { kind, durationSeconds: 300 };
  }
};

// Kind-narrowed updates: a patch can only land on a step that has the field.
const withDuration = (step: WorkoutStep, durationSeconds: number): WorkoutStep =>
  step.kind === "repeat" ? step : { ...step, durationSeconds };
const withSteadyTarget = (step: WorkoutStep, target: PowerTarget): WorkoutStep =>
  step.kind === "steady" ? { ...step, target } : step;
const withRampStart = (step: WorkoutStep, start: PowerTarget): WorkoutStep =>
  step.kind === "ramp" ? { ...step, start } : step;
const withRampEnd = (step: WorkoutStep, end: PowerTarget): WorkoutStep =>
  step.kind === "ramp" ? { ...step, end } : step;

const stepLabel = (step: WorkoutStep) => (step.kind === "freeRide" ? "FREE" : step.kind.toUpperCase());

export function WorkoutEditor({
  initial,
  ftpWatts,
  powerZones,
  close,
  save,
}: {
  initial: Workout;
  ftpWatts: number;
  powerZones: readonly ZoneDefinition[];
  close: () => void;
  save: (workout: Workout) => void;
}) {
  const [state, setState] = useState(() => toEditorState(initial));
  const [highlightedRowId, setHighlightedRowId] = useState<string | null>(null);
  const rowElements = useRef(new Map<string, HTMLDivElement>());
  const dialogRef = useDialog<HTMLDivElement>(close);

  const steps = useMemo(() => state.rows.map((row) => row.step), [state.rows]);
  const stats = useMemo(
    () => profileStats(profileSegments(steps, ftpWatts), ftpWatts),
    [steps, ftpWatts],
  );

  const setMeta = (patch: Partial<EditorState["meta"]>) =>
    setState((current) => ({ ...current, meta: { ...current.meta, ...patch } }));
  const updateRow = (id: string, update: (step: WorkoutStep) => WorkoutStep) =>
    setState((current) => ({
      ...current,
      rows: current.rows.map((row) => (row.id === id ? { ...row, step: update(row.step) } : row)),
    }));
  const removeRow = (id: string) => {
    setState((current) => ({ ...current, rows: current.rows.filter((row) => row.id !== id) }));
    setHighlightedRowId((current) => (current === id ? null : current));
  };
  const addStep = (kind: LeafStep["kind"]) =>
    setState((current) => ({
      ...current,
      rows: [...current.rows, { id: newRowId(), step: defaultStep(kind) }],
    }));

  // Top-level rows map to single-element paths. Nested paths will resolve
  // through the same rows once repeat groups become editable.
  const highlightedPath = useMemo<StepPath | null>(() => {
    const index = state.rows.findIndex((row) => row.id === highlightedRowId);
    return index === -1 ? null : [index];
  }, [state.rows, highlightedRowId]);
  const rowIdAtPath = useCallback(
    (path: StepPath | null) => (path === null ? null : state.rows[path[0]]?.id ?? null),
    [state.rows],
  );
  const highlightFromChart = useCallback(
    (path: StepPath | null) => setHighlightedRowId(rowIdAtPath(path)),
    [rowIdAtPath],
  );
  const selectFromChart = useCallback(
    (path: StepPath) => {
      const id = rowIdAtPath(path);
      const element = id === null ? undefined : rowElements.current.get(id);
      if (!element) return;
      element.scrollIntoView?.({ block: "nearest" });
      element.querySelector<HTMLInputElement>("input:not([disabled])")?.focus();
    },
    [rowIdAtPath],
  );

  const workout = toWorkout(state);
  const canSave = workout.name.trim().length > 0 && workout.steps.length > 0;

  return (
    <div
      className="modal-backdrop dialog-enter"
      role="presentation"
      onMouseDown={(event) => { if (event.target === event.currentTarget) close(); }}
    >
      <div
        ref={dialogRef}
        className="modal editor-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="workout-builder-heading"
        tabIndex={-1}
      >
        <button className="modal-close" onClick={close} aria-label="Close workout builder"><X /></button>
        <span className="label">WORKOUT BUILDER</span>
        <h2 id="workout-builder-heading" className="sr-only">Workout builder</h2>
        <div className="editor-title">
          <label className="sr-only" htmlFor="workout-builder-title">Workout name</label>
          <input
            id="workout-builder-title"
            aria-label="Workout name"
            value={state.meta.name}
            onChange={(event) => setMeta({ name: event.target.value })}
          />
          <strong>{formatDuration(stats.totalSeconds)}</strong>
        </div>
        <textarea
          aria-label="Workout description"
          placeholder="Workout description"
          value={state.meta.description}
          onChange={(event) => setMeta({ description: event.target.value })}
        />

        <section className="editor-profile" aria-label="Workout preview">
          <WorkoutProfile
            variant="editor"
            steps={steps}
            ftpWatts={ftpWatts}
            powerZones={powerZones}
            highlightedPath={highlightedPath}
            onHighlightPath={highlightFromChart}
            onSelectPath={selectFromChart}
          />
          <div className="editor-stats">
            <Metric value={formatDuration(stats.totalSeconds)} unit="duration" />
            <Metric
              value={stats.averagePercentFtp === null ? "—" : `${Math.round(stats.averagePercentFtp)}%`}
              unit="avg FTP"
            />
            <Metric
              value={stats.estimatedStress === null ? "—" : `${Math.round(stats.estimatedStress)}`}
              unit="est. stress"
            />
            <Metric value={`${stats.blockCount}`} unit="blocks" />
            {stats.freeRideSeconds > 0 && (
              <span className="editor-stats-note">
                {formatDuration(stats.freeRideSeconds)} of free ride is not counted in intensity
              </span>
            )}
          </div>
        </section>

        <div className="step-list">
          {state.rows.map((row, index) => {
            const { step } = row;
            return (
              <div
                className="step-editor"
                key={row.id}
                data-row-id={row.id}
                data-highlighted={highlightedRowId === row.id ? "true" : undefined}
                ref={(element) => {
                  if (element) rowElements.current.set(row.id, element);
                  else rowElements.current.delete(row.id);
                }}
                onMouseEnter={() => setHighlightedRowId(row.id)}
                onMouseLeave={() => setHighlightedRowId((current) => (current === row.id ? null : current))}
                onFocus={() => setHighlightedRowId(row.id)}
                onBlur={(event) => {
                  if (!event.currentTarget.contains(event.relatedTarget as Node | null)) {
                    setHighlightedRowId((current) => (current === row.id ? null : current));
                  }
                }}
              >
                <span className={`step-kind ${step.kind}`}>{stepLabel(step)}</span>
                <label>
                  Duration (sec)
                  <EditableNumberInput
                    min={1}
                    value={step.kind === "repeat" ? workoutDuration(step.steps) : step.durationSeconds}
                    disabled={step.kind === "repeat"}
                    onValueChange={(durationSeconds) => updateRow(row.id, (current) => withDuration(current, durationSeconds))}
                  />
                </label>
                {step.kind === "steady" && (
                  <TargetInput
                    label="Power"
                    target={step.target}
                    onChange={(target) => updateRow(row.id, (current) => withSteadyTarget(current, target))}
                  />
                )}
                {step.kind === "ramp" && (
                  <>
                    <TargetInput
                      label="Start"
                      target={step.start}
                      onChange={(start) => updateRow(row.id, (current) => withRampStart(current, start))}
                    />
                    <TargetInput
                      label="End"
                      target={step.end}
                      onChange={(end) => updateRow(row.id, (current) => withRampEnd(current, end))}
                    />
                  </>
                )}
                {step.kind === "repeat" && (
                  <span className="repeat-summary">{step.repetitions}× repeat group</span>
                )}
                <button
                  className="icon-button danger"
                  aria-label={`Remove block ${index + 1}`}
                  onClick={() => removeRow(row.id)}
                >
                  <Trash2 size={16} />
                </button>
              </div>
            );
          })}
        </div>
        <div className="add-steps">
          <span>Add block</span>
          <button onClick={() => addStep("steady")}><Plus />Steady</button>
          <button onClick={() => addStep("ramp")}><Plus />Ramp</button>
          <button onClick={() => addStep("freeRide")}><Plus />Free ride</button>
        </div>
        <div className="editor-actions">
          <button className="secondary" onClick={close}>Cancel</button>
          <button className="primary" disabled={!canSave} onClick={() => save(workout)}>Save workout</button>
        </div>
      </div>
    </div>
  );
}

function TargetInput({
  label,
  target,
  onChange,
}: {
  label: string;
  target: PowerTarget;
  onChange: (target: PowerTarget) => void;
}) {
  // Keep whatever unit the step already uses; imported workouts may target watts.
  const watts = target.unit === "watts";
  return (
    <label>
      {label} ({watts ? "W" : "% FTP"})
      <EditableNumberInput
        min={1}
        max={watts ? 2000 : 300}
        value={target.value}
        onValueChange={(value) => onChange({ unit: target.unit, value })}
      />
    </label>
  );
}
