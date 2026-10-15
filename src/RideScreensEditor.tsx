import { useEffect, useState } from "react";
import { ChevronDown, ChevronUp, Plus, Trash2 } from "lucide-react";
import { EditableNumberInput } from "./EditableNumberInput";
import {
  LAP_TIMER_REST_SECONDS,
  LAP_TIMER_WORK_SECONDS,
  defaultLapTimerSettings,
  isLapTimerMode,
  lapTimerModeLabels,
  lapTimerModes,
  normalizeLapTimerSettings,
  type LapTimerSettings,
} from "./lapTimer";
import {
  firstRideField,
  rideAggregatesFor,
  rideFieldChoiceLabel,
  rideFieldKey,
  rideFieldLabel,
  rideFields,
  rideMetricLabels,
  rideMetrics,
  ridePanelIds,
  ridePanelLabels,
  rideScopeLabels,
  rideScopesFor,
  type RideField,
  type RideMetric,
  type RidePanelId,
  type RideScope,
} from "./rideFields";
import {
  MAX_RIDE_SCREENS,
  defaultRideDisplayPreferences,
  rideScreenItemKey,
  rideSlotSpans,
  type RideDisplayPreferences,
  type RideScreen,
  type RideScreenItem,
  type RideSlotSpan,
} from "./rideScreens";

const spanLabels: Record<RideSlotSpan, string> = {
  1: "Small",
  2: "Wide",
  4: "Full width",
};

const usedKeys = (screen: RideScreen): Set<string> =>
  new Set(screen.items.map(rideScreenItemKey));

const hasLapTimer = (screen: RideScreen): boolean =>
  screen.items.some((item) => item.kind === "lapTimer");

/** The first field this screen does not already show. */
const nextField = (screen: RideScreen): RideField | null => {
  const used = usedKeys(screen);
  return rideFields.find((field) => !used.has(`field:${rideFieldKey(field)}`)) ?? null;
};

const nextPanel = (screen: RideScreen): RidePanelId | null => {
  const used = usedKeys(screen);
  return ridePanelIds.find((panel) => !used.has(`panel:${panel}`)) ?? null;
};

const move = <T,>(items: T[], index: number, direction: -1 | 1): T[] => {
  const target = index + direction;
  if (target < 0 || target >= items.length) return items;
  const next = [...items];
  [next[index], next[target]] = [next[target], next[index]];
  return next;
};

/**
 * Settings → Ride screens. Screens hold slots; a slot is either a field
 * (metric, over what, which way) or one of the richer panels.
 */
export function RideScreensEditor({
  preferences,
  onSave,
}: {
  preferences: RideDisplayPreferences;
  onSave: (preferences: RideDisplayPreferences) => void;
}) {
  const [draft, setDraft] = useState(preferences);
  useEffect(() => setDraft(preferences), [preferences]);

  const setScreens = (screens: RideScreen[]) => setDraft({ version: 3, screens });

  const updateScreen = (index: number, update: (screen: RideScreen) => RideScreen) =>
    setScreens(draft.screens.map((screen, current) => (current === index ? update(screen) : screen)));

  const updateItems = (index: number, update: (items: RideScreenItem[]) => RideScreenItem[]) =>
    updateScreen(index, (screen) => ({ ...screen, items: update(screen.items) }));

  const addScreen = () =>
    setScreens([
      ...draft.screens,
      {
        id: `screen-${Date.now()}`,
        name: `Screen ${draft.screens.length + 1}`,
        items: [{ kind: "field", field: firstRideField("power"), span: 1 }],
      },
    ]);

  return (
    <section className="card settings-card">
      <div>
        <span className="label">RIDE LAYOUT</span>
        <h2>Ride screens</h2>
        <p>
          Build the screens you ride by. Each screen holds fields — a metric, over the whole ride or
          just the current block, live or averaged — the larger panels like the workout timeline
          and the charts, and a lap timer for drills. Page between screens while riding with the
          tabs or the ← and → keys; press L to mark a lap.
        </p>
      </div>
      <div className="ride-layout-settings">
        {draft.screens.map((screen, screenIndex) => (
          <div className="ride-screen-setting" key={screen.id}>
            <div className="ride-screen-head">
              <label>
                <span className="sr-only">Screen name</span>
                <input
                  value={screen.name}
                  aria-label={`Name of screen ${screenIndex + 1}`}
                  onChange={(event) =>
                    updateScreen(screenIndex, (current) => ({ ...current, name: event.target.value }))
                  }
                />
              </label>
              <div className="ride-card-order">
                <button
                  type="button"
                  className="icon-button"
                  aria-label={`Move ${screen.name} up`}
                  disabled={screenIndex === 0}
                  onClick={() => setScreens(move(draft.screens, screenIndex, -1))}
                >
                  <ChevronUp size={17} />
                </button>
                <button
                  type="button"
                  className="icon-button"
                  aria-label={`Move ${screen.name} down`}
                  disabled={screenIndex === draft.screens.length - 1}
                  onClick={() => setScreens(move(draft.screens, screenIndex, 1))}
                >
                  <ChevronDown size={17} />
                </button>
                <button
                  type="button"
                  className="icon-button"
                  aria-label={`Remove ${screen.name}`}
                  // A rider always has somewhere to ride.
                  disabled={draft.screens.length === 1}
                  onClick={() =>
                    setScreens(draft.screens.filter((_, index) => index !== screenIndex))
                  }
                >
                  <Trash2 size={17} />
                </button>
              </div>
            </div>
            <div className="ride-card-list">
              {screen.items.map((item, itemIndex) => {
                const replace = (next: RideScreenItem) =>
                  updateItems(screenIndex, (items) =>
                    items.map((current, index) => (index === itemIndex ? next : current)),
                  );
                return (
                <div className="ride-card-setting" key={rideScreenItemKey(item)}>
                  {item.kind === "field" ? (
                    <FieldRow field={item.field} span={item.span} onChange={replace} />
                  ) : item.kind === "lapTimer" ? (
                    <LapTimerRow
                      timer={item.timer}
                      onChange={(timer) => replace({ kind: "lapTimer", timer })}
                    />
                  ) : (
                    <div className="ride-slot-controls">
                      <span className="ride-slot-kind">Panel</span>
                      <label>
                        <span className="sr-only">Panel</span>
                        <select
                          value={item.panel}
                          onChange={(event) => {
                            const panel = ridePanelIds.find((id) => id === event.target.value);
                            if (panel !== undefined) replace({ kind: "panel", panel });
                          }}
                        >
                          {ridePanelIds.map((panel) => (
                            <option key={panel} value={panel}>
                              {ridePanelLabels[panel]}
                            </option>
                          ))}
                        </select>
                      </label>
                    </div>
                  )}
                  <div className="ride-card-order">
                    <button
                      type="button"
                      className="icon-button"
                      aria-label={`Move slot ${itemIndex + 1} of ${screen.name} up`}
                      disabled={itemIndex === 0}
                      onClick={() => updateItems(screenIndex, (items) => move(items, itemIndex, -1))}
                    >
                      <ChevronUp size={17} />
                    </button>
                    <button
                      type="button"
                      className="icon-button"
                      aria-label={`Move slot ${itemIndex + 1} of ${screen.name} down`}
                      disabled={itemIndex === screen.items.length - 1}
                      onClick={() => updateItems(screenIndex, (items) => move(items, itemIndex, 1))}
                    >
                      <ChevronDown size={17} />
                    </button>
                    <button
                      type="button"
                      className="icon-button"
                      aria-label={`Remove slot ${itemIndex + 1} of ${screen.name}`}
                      onClick={() =>
                        updateItems(screenIndex, (items) =>
                          items.filter((_, index) => index !== itemIndex),
                        )
                      }
                    >
                      <Trash2 size={17} />
                    </button>
                  </div>
                </div>
                );
              })}
            </div>
            <div className="ride-screen-add">
              <button
                type="button"
                className="secondary"
                disabled={nextField(screen) === null}
                onClick={() => {
                  const field = nextField(screen);
                  if (field === null) return;
                  updateItems(screenIndex, (items) => [...items, { kind: "field", field, span: 1 }]);
                }}
              >
                <Plus size={15} /> Add field
              </button>
              <button
                type="button"
                className="secondary"
                disabled={nextPanel(screen) === null}
                onClick={() => {
                  const panel = nextPanel(screen);
                  if (panel === undefined || panel === null) return;
                  updateItems(screenIndex, (items) => [...items, { kind: "panel", panel }]);
                }}
              >
                <Plus size={15} /> Add panel
              </button>
              <button
                type="button"
                className="secondary"
                disabled={hasLapTimer(screen)}
                onClick={() =>
                  updateItems(screenIndex, (items) => [
                    ...items,
                    { kind: "lapTimer", timer: defaultLapTimerSettings },
                  ])
                }
              >
                <Plus size={15} /> Add lap timer
              </button>
            </div>
          </div>
        ))}
        <div className="settings-actions">
          <button
            type="button"
            className="secondary"
            disabled={draft.screens.length >= MAX_RIDE_SCREENS}
            onClick={addScreen}
          >
            <Plus size={15} /> Add screen
          </button>
          <button
            type="button"
            className="secondary"
            onClick={() => setDraft(defaultRideDisplayPreferences)}
          >
            Reset to default
          </button>
          <button type="button" className="primary" onClick={() => onSave(draft)}>
            Save ride layout
          </button>
        </div>
      </div>
    </section>
  );
}

/** Metric, scope and aggregate pickers that can only produce a field that exists. */
function FieldRow({
  field,
  span,
  onChange,
}: {
  field: RideField;
  span: RideSlotSpan;
  onChange: (item: RideScreenItem) => void;
}) {
  const scopes = rideScopesFor(field.metric);
  const aggregates = rideAggregatesFor(field.metric, field.scope);
  const label = rideFieldLabel(field) ?? "";

  const change = (next: RideField) => onChange({ kind: "field", field: next, span });

  return (
    <div className="ride-slot-controls">
      <span className="ride-slot-kind" title={label}>
        Field
      </span>
      <label>
        <span className="sr-only">Metric</span>
        <select
          value={field.metric}
          onChange={(event) => change(firstRideField(event.target.value as RideMetric))}
        >
          {rideMetrics.map((metric) => (
            <option key={metric} value={metric}>
              {rideMetricLabels[metric]}
            </option>
          ))}
        </select>
      </label>
      <label>
        <span className="sr-only">Measured over</span>
        <select
          value={field.scope}
          disabled={scopes.length === 1}
          onChange={(event) => {
            const scope = event.target.value as RideScope;
            const next = rideAggregatesFor(field.metric, scope);
            change({
              metric: field.metric,
              scope,
              aggregate: next.includes(field.aggregate) ? field.aggregate : next[0],
            });
          }}
        >
          {scopes.map((scope) => (
            <option key={scope} value={scope}>
              {rideScopeLabels[scope]}
            </option>
          ))}
        </select>
      </label>
      <label>
        <span className="sr-only">Shown as</span>
        <select
          value={field.aggregate}
          disabled={aggregates.length === 1}
          onChange={(event) => {
            const aggregate = aggregates.find((option) => option === event.target.value);
            if (aggregate !== undefined) change({ ...field, aggregate });
          }}
        >
          {aggregates.map((aggregate) => (
            <option key={aggregate} value={aggregate}>
              {rideFieldChoiceLabel({ ...field, aggregate })}
            </option>
          ))}
        </select>
      </label>
      <label>
        <span className="sr-only">Size</span>
        <select
          value={span}
          onChange={(event) =>
            onChange({ kind: "field", field, span: Number(event.target.value) as RideSlotSpan })
          }
        >
          {rideSlotSpans.map((option) => (
            <option key={option} value={option}>
              {spanLabels[option]}
            </option>
          ))}
        </select>
      </label>
    </div>
  );
}

/** Manual laps, or a countdown drill that repeats work (and optional rest) and restarts at each lap. */
function LapTimerRow({
  timer,
  onChange,
}: {
  timer: LapTimerSettings;
  onChange: (timer: LapTimerSettings) => void;
}) {
  const change = (next: Partial<LapTimerSettings>) =>
    onChange(normalizeLapTimerSettings({ ...timer, ...next }));
  return (
    <div className="ride-slot-controls lap-timer-setting">
      <span className="ride-slot-kind">Lap timer</span>
      <label>
        <span className="sr-only">Lap timer mode</span>
        <select
          value={timer.mode}
          onChange={(event) => {
            const mode = event.target.value;
            if (isLapTimerMode(mode)) change({ mode });
          }}
        >
          {lapTimerModes.map((mode) => (
            <option key={mode} value={mode}>
              {lapTimerModeLabels[mode]}
            </option>
          ))}
        </select>
      </label>
      {timer.mode === "countdown" && (
        <>
          <label className="lap-timer-seconds">
            <span>On</span>
            <EditableNumberInput
              aria-label="Drill work seconds"
              value={timer.workSeconds}
              min={LAP_TIMER_WORK_SECONDS.min}
              max={LAP_TIMER_WORK_SECONDS.max}
              step={5}
              onValueChange={(workSeconds) => change({ workSeconds })}
            />
            <span>s</span>
          </label>
          <label className="lap-timer-seconds">
            <span>Off</span>
            <EditableNumberInput
              aria-label="Drill rest seconds"
              value={timer.restSeconds}
              min={LAP_TIMER_REST_SECONDS.min}
              max={LAP_TIMER_REST_SECONDS.max}
              step={5}
              onValueChange={(restSeconds) => change({ restSeconds })}
            />
            <span>s</span>
          </label>
        </>
      )}
    </div>
  );
}
