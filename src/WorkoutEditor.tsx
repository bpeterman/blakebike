import { useCallback, useMemo, useRef, useState, type FocusEvent } from "react";
import { ChevronDown, ChevronUp, Plus, Repeat, Trash2, Ungroup, X } from "lucide-react";
import type { PowerTarget, Workout, ZoneDefinition } from "./types";
import { formatDuration, workoutDuration } from "./types";
import { EditableNumberInput } from "./EditableNumberInput";
import { Metric } from "./Metric";
import { WorkoutProfile } from "./WorkoutProfile";
import { useDialog } from "./useDialog";
import { profileSegments, profileStats } from "./workoutProfileModel";
import {
  appendNode,
  canMoveNode,
  formatStepPath,
  fromStepNodes,
  groupNodes,
  MAX_REPETITIONS,
  MIN_REPETITIONS,
  moveNode,
  nodeAtPath,
  nodePath,
  removeNode,
  stepsProblem,
  toStepNodes,
  ungroupNode,
  updateNode,
  type LeafStep,
  type MoveDirection,
  type StepNode,
  type StepPath,
} from "./workoutSteps";

/**
 * The steps live in a tree whose nodes carry ids that survive reordering,
 * grouping, and deletion, at any depth. Ids exist only while the editor is
 * open; `toWorkout` strips them.
 */
type EditorState = {
  meta: Omit<Workout, "steps">;
  nodes: StepNode[];
};

let nextNodeId = 0;
const newNodeId = () => `step-${(nextNodeId += 1)}`;

const toEditorState = (workout: Workout): EditorState => {
  const { steps, ...meta } = structuredClone(workout);
  return { meta, nodes: toStepNodes(steps, newNodeId) };
};

const defaultStep = (kind: LeafStep["kind"]): LeafStep => {
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

const leafNode = (step: LeafStep): StepNode => ({ id: newNodeId(), kind: "leaf", step });

/** A fresh repeat starts as a classic work/recovery pair so it is useful straight away. */
const defaultRepeat = (): StepNode => ({
  id: newNodeId(),
  kind: "repeat",
  repetitions: 4,
  children: [
    leafNode({ kind: "steady", durationSeconds: 60, target: { unit: "percentFtp", value: 110 } }),
    leafNode({ kind: "steady", durationSeconds: 60, target: { unit: "percentFtp", value: 55 } }),
  ],
});

// Kind-narrowed updates: a patch can only land on a node that has the field.
const withLeaf = (node: StepNode, update: (step: LeafStep) => LeafStep): StepNode =>
  node.kind === "leaf" ? { ...node, step: update(node.step) } : node;
const withRepetitions = (node: StepNode, repetitions: number): StepNode =>
  node.kind === "repeat" ? { ...node, repetitions } : node;
const withDuration = (step: LeafStep, durationSeconds: number): LeafStep => ({ ...step, durationSeconds });
const withSteadyTarget = (step: LeafStep, target: PowerTarget): LeafStep =>
  step.kind === "steady" ? { ...step, target } : step;
const withRampStart = (step: LeafStep, start: PowerTarget): LeafStep =>
  step.kind === "ramp" ? { ...step, start } : step;
const withRampEnd = (step: LeafStep, end: PowerTarget): LeafStep =>
  step.kind === "ramp" ? { ...step, end } : step;

const stepLabel = (step: LeafStep) => (step.kind === "freeRide" ? "FREE" : step.kind.toUpperCase());

const LEAF_KINDS: readonly { kind: LeafStep["kind"]; label: string }[] = [
  { kind: "steady", label: "Steady" },
  { kind: "ramp", label: "Ramp" },
  { kind: "freeRide", label: "Free ride" },
];

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
  const [highlightedId, setHighlightedId] = useState<string | null>(null);
  const [selectedIds, setSelectedIds] = useState<ReadonlySet<string>>(() => new Set());
  const rowElements = useRef(new Map<string, HTMLDivElement>());
  const dialogRef = useDialog<HTMLDivElement>(close);
  const { nodes } = state;

  const steps = useMemo(() => fromStepNodes(nodes), [nodes]);
  const stats = useMemo(
    () => profileStats(profileSegments(steps, ftpWatts), ftpWatts),
    [steps, ftpWatts],
  );
  const problem = useMemo(() => stepsProblem(steps), [steps]);

  const setMeta = (patch: Partial<EditorState["meta"]>) =>
    setState((current) => ({ ...current, meta: { ...current.meta, ...patch } }));
  const editNodes = (edit: (nodes: StepNode[]) => StepNode[]) =>
    setState((current) => ({ ...current, nodes: edit(current.nodes) }));
  const updateLeaf = (id: string, update: (step: LeafStep) => LeafStep) =>
    editNodes((current) => updateNode(current, id, (node) => withLeaf(node, update)));
  const forget = (id: string) => {
    setHighlightedId((current) => (current === id ? null : current));
    setSelectedIds((current) => {
      if (!current.has(id)) return current;
      const next = new Set(current);
      next.delete(id);
      return next;
    });
  };
  const remove = (id: string) => {
    editNodes((current) => removeNode(current, id));
    forget(id);
  };
  const ungroup = (id: string) => {
    editNodes((current) => ungroupNode(current, id));
    forget(id);
  };
  const move = (id: string, direction: MoveDirection) => editNodes((current) => moveNode(current, id, direction));
  const append = (parentId: string | null, node: StepNode) =>
    editNodes((current) => appendNode(current, parentId, node));
  const toggleSelected = (id: string) =>
    setSelectedIds((current) => {
      const next = new Set(current);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  const group = (ids: readonly string[]) => {
    const repeat = { id: newNodeId(), repetitions: MIN_REPETITIONS };
    // Validity is checked against the tree being rendered, so the update can trust it.
    if (groupNodes(nodes, ids, repeat) === null) return;
    editNodes((current) => groupNodes(current, ids, repeat) ?? current);
    setSelectedIds(new Set());
  };

  // A selection only counts while its rows still exist; it can be grouped
  // when the rows are neighbours under the same parent.
  const selection = useMemo(
    () => [...selectedIds].filter((id) => nodePath(nodes, id) !== null),
    [selectedIds, nodes],
  );
  const selectionGroupable = useMemo(
    () => selection.length > 0 && groupNodes(nodes, selection, { id: "", repetitions: MIN_REPETITIONS }) !== null,
    [selection, nodes],
  );

  const highlightedPath = useMemo<StepPath | null>(
    () => (highlightedId === null ? null : nodePath(nodes, highlightedId)),
    [nodes, highlightedId],
  );
  const idAtPath = useCallback(
    (path: StepPath | null) => (path === null ? null : nodeAtPath(nodes, path)?.id ?? null),
    [nodes],
  );
  const highlightFromChart = useCallback(
    (path: StepPath | null) => setHighlightedId(idAtPath(path)),
    [idAtPath],
  );
  const selectFromChart = useCallback(
    (path: StepPath) => {
      const id = idAtPath(path);
      const element = id === null ? undefined : rowElements.current.get(id);
      if (!element) return;
      element.scrollIntoView?.({ block: "nearest" });
      element.querySelector<HTMLInputElement>('input[type="number"]:not([disabled])')?.focus();
    },
    [idAtPath],
  );

  const workout: Workout = { ...state.meta, steps };
  const canSave = workout.name.trim().length > 0 && workout.steps.length > 0 && problem === null;

  // Hover and focus live on each row, never on a repeat's whole body, so a
  // child row highlights itself rather than its group.
  const rowProps = (id: string) => ({
    "data-row-id": id,
    "data-highlighted": highlightedId === id ? "true" : undefined,
    ref: (element: HTMLDivElement | null) => {
      if (element) rowElements.current.set(id, element);
      else rowElements.current.delete(id);
    },
    onMouseEnter: () => setHighlightedId(id),
    onMouseLeave: () => setHighlightedId((current) => (current === id ? null : current)),
    onFocus: () => setHighlightedId(id),
    onBlur: (event: FocusEvent<HTMLDivElement>) => {
      const next = event.relatedTarget;
      if (!(next instanceof Node && event.currentTarget.contains(next))) {
        setHighlightedId((current) => (current === id ? null : current));
      }
    },
  });

  const rowActions = (node: StepNode, label: string) => (
    <div className="step-actions">
      <button
        className="icon-button"
        aria-label={`Move block ${label} up`}
        disabled={!canMoveNode(nodes, node.id, -1)}
        onClick={() => move(node.id, -1)}
      >
        <ChevronUp size={16} />
      </button>
      <button
        className="icon-button"
        aria-label={`Move block ${label} down`}
        disabled={!canMoveNode(nodes, node.id, 1)}
        onClick={() => move(node.id, 1)}
      >
        <ChevronDown size={16} />
      </button>
      {node.kind === "leaf" ? (
        <button className="icon-button" aria-label={`Repeat block ${label}`} onClick={() => group([node.id])}>
          <Repeat size={16} />
        </button>
      ) : (
        <button className="icon-button" aria-label={`Ungroup block ${label}`} onClick={() => ungroup(node.id)}>
          <Ungroup size={16} />
        </button>
      )}
      <button className="icon-button danger" aria-label={`Remove block ${label}`} onClick={() => remove(node.id)}>
        <Trash2 size={16} />
      </button>
    </div>
  );

  const selectBox = (id: string, label: string) => (
    <input
      type="checkbox"
      className="step-select"
      aria-label={`Select block ${label}`}
      checked={selectedIds.has(id)}
      onChange={() => toggleSelected(id)}
    />
  );

  const renderNodes = (list: readonly StepNode[], prefix: StepPath) =>
    list.map((node, index) => {
      const label = formatStepPath([...prefix, index]);
      if (node.kind === "repeat") {
        const once = workoutDuration(fromStepNodes(node.children));
        return (
          <div className="repeat-group" key={node.id} data-group-id={node.id}>
            <div className="step-editor repeat-header" {...rowProps(node.id)}>
              {selectBox(node.id, label)}
              <span className="step-kind repeat">REPEAT</span>
              <div className="step-fields">
                <label>
                  Repeats
                  <EditableNumberInput
                    min={MIN_REPETITIONS}
                    max={MAX_REPETITIONS}
                    step={1}
                    value={node.repetitions}
                    onValueChange={(repetitions) =>
                      editNodes((current) => updateNode(current, node.id, (found) => withRepetitions(found, repetitions)))
                    }
                  />
                </label>
                <span className="repeat-summary">
                  {node.repetitions} × {formatDuration(once)} = {formatDuration(node.repetitions * once)}
                </span>
              </div>
              {rowActions(node, label)}
            </div>
            <div className="repeat-children">
              {node.children.length === 0 ? (
                <p className="repeat-empty">This repeat is empty. Add a step or remove it.</p>
              ) : (
                renderNodes(node.children, [...prefix, index])
              )}
              <div className="add-steps nested">
                <span>Add to repeat</span>
                {LEAF_KINDS.map(({ kind, label: kindLabel }) => (
                  <button
                    key={kind}
                    aria-label={`Add ${kindLabel.toLowerCase()} to repeat ${label}`}
                    onClick={() => append(node.id, leafNode(defaultStep(kind)))}
                  >
                    <Plus />{kindLabel}
                  </button>
                ))}
              </div>
            </div>
          </div>
        );
      }
      const { step } = node;
      return (
        <div className="step-editor" key={node.id} {...rowProps(node.id)}>
          {selectBox(node.id, label)}
          <span className={`step-kind ${step.kind}`}>{stepLabel(step)}</span>
          <div className="step-fields">
            <label>
              Duration (sec)
              <EditableNumberInput
                min={1}
                value={step.durationSeconds}
                onValueChange={(durationSeconds) => updateLeaf(node.id, (current) => withDuration(current, durationSeconds))}
              />
            </label>
            {step.kind === "steady" && (
              <TargetInput
                label="Power"
                target={step.target}
                onChange={(target) => updateLeaf(node.id, (current) => withSteadyTarget(current, target))}
              />
            )}
            {step.kind === "ramp" && (
              <>
                <TargetInput
                  label="Start"
                  target={step.start}
                  onChange={(start) => updateLeaf(node.id, (current) => withRampStart(current, start))}
                />
                <TargetInput
                  label="End"
                  target={step.end}
                  onChange={(end) => updateLeaf(node.id, (current) => withRampEnd(current, end))}
                />
              </>
            )}
          </div>
          {rowActions(node, label)}
        </div>
      );
    });

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

        <div className="step-list">{renderNodes(nodes, [])}</div>
        <div className="add-steps">
          <span>Add block</span>
          {selection.length > 0 && (
            <button
              className="group-selection"
              disabled={!selectionGroupable}
              title={selectionGroupable ? undefined : "Select neighbouring blocks at the same level to repeat them"}
              onClick={() => group(selection)}
            >
              <Repeat />Repeat {selection.length} selected
            </button>
          )}
          {LEAF_KINDS.map(({ kind, label }) => (
            <button key={kind} onClick={() => append(null, leafNode(defaultStep(kind)))}><Plus />{label}</button>
          ))}
          <button onClick={() => append(null, defaultRepeat())}><Plus />Repeat</button>
        </div>
        <div className="editor-actions">
          {problem !== null && <span className="editor-problem" role="status">{problem}</span>}
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
