/**
 * The ride display: what a rider can put on a ride screen, and how each of
 * those numbers is worked out.
 *
 * A *field* is a descriptor — `{ metric, scope, aggregate }` — rather than a
 * card type of its own, so power, average power, block average power and max
 * power are one metric with modifiers. A *panel* is one of the richer cards
 * (charts, the workout timeline) that is not a single number. A ride screen is
 * an ordered list of both, and a rider can have several screens and page
 * between them.
 */
import {
  formatDistance,
  formatDuration,
  formatSpeed,
  kilojoulesToKilocalories,
  peak,
  timeWeightedMean,
  workKilojoules,
  type Profile,
  type Telemetry,
} from "./types";

export type RideMetric =
  | "power"
  | "cadence"
  | "heartRate"
  | "speed"
  | "wattsPerKilogram"
  | "distance"
  | "energy"
  | "calories"
  | "time"
  | "targetPower";

/** "interval" is the workout block being ridden — this app's version of a lap. */
export type RideScope = "ride" | "interval";

export type RideAggregate = "current" | "average" | "max" | "total" | "remaining";

export type RideField = {
  metric: RideMetric;
  scope: RideScope;
  aggregate: RideAggregate;
};

/** Stable identity for a field, used for React keys and lookups. */
export const rideFieldKey = (field: RideField): string =>
  `${field.metric}:${field.scope}:${field.aggregate}`;

/** Cards that are not a single number and keep their own component. */
export const ridePanelIds = [
  "workoutTimeline",
  "targetAndBias",
  "powerChart",
  "heartRateChart",
  "timeInZone",
  "deviceStats",
] as const;

export type RidePanelId = (typeof ridePanelIds)[number];

export const ridePanelLabels: Record<RidePanelId, string> = {
  workoutTimeline: "Workout timeline",
  targetAndBias: "Target & bias",
  powerChart: "Power chart",
  heartRateChart: "Heart-rate chart",
  timeInZone: "Time in zone",
  deviceStats: "Stats for nerds",
};

export const isRidePanelId = (value: string): value is RidePanelId =>
  (ridePanelIds as readonly string[]).includes(value);

/** Everything a field needs to resolve itself. Built once per render. */
export type RideFieldContext = {
  telemetry: Telemetry;
  /** The whole ride, oldest sample first. */
  history: Telemetry[];
  /** Just the block being ridden; the whole ride on a free ride. */
  intervalHistory: Telemetry[];
  /** Power as the ride screen shows it, after the rider's smoothing choice. */
  displayPowerWatts: number;
  elapsedSeconds: number;
  totalSeconds: number | null;
  intervalElapsedSeconds: number;
  /** Null on a free-ride block, which has no planned end. */
  intervalTotalSeconds: number | null;
  targetPowerWatts: number | null;
  distanceMeters: number;
  profile: Profile;
};

export type RideFieldValue = {
  /** Already upper-cased for the tile heading. */
  label: string;
  value: string;
  unit?: string;
  note?: string;
};

type Resolver = (context: RideFieldContext) => Omit<RideFieldValue, "label"> | null;

type Definition = { label: string; resolve: Resolver };

const MISSING = "—";

const scopeLabel = (scope: RideScope): string => (scope === "interval" ? "BLOCK " : "");

const aggregateLabel = (aggregate: RideAggregate): string =>
  aggregate === "average" ? "AVG " : aggregate === "max" ? "MAX " : "";

const scopedHistory = (scope: RideScope, context: RideFieldContext): Telemetry[] =>
  scope === "interval" ? context.intervalHistory : context.history;

/**
 * A metric read from every sample, so it can be shown live, averaged over
 * time, or peaked. `live` is what the rider is doing right now, which for
 * power is the smoothed figure the power tile shows.
 */
type SampledMetric = {
  metric: RideMetric;
  label: string;
  live: (context: RideFieldContext) => number | null;
  sample: (sample: Telemetry, context: RideFieldContext) => number | null;
  format: (value: number, context: RideFieldContext) => { value: string; unit: string };
};

const whole = (unit: string) => (value: number) => ({ value: String(Math.round(value)), unit });

const sampledMetrics: SampledMetric[] = [
  {
    metric: "power",
    label: "POWER",
    live: (context) => context.displayPowerWatts,
    sample: (sample) => sample.powerWatts,
    format: whole("W"),
  },
  {
    metric: "cadence",
    label: "CADENCE",
    live: (context) => context.telemetry.cadenceRpm,
    sample: (sample) => sample.cadenceRpm,
    format: whole("rpm"),
  },
  {
    metric: "heartRate",
    label: "HEART RATE",
    live: (context) => context.telemetry.heartRateBpm,
    sample: (sample) => sample.heartRateBpm,
    format: whole("bpm"),
  },
  {
    metric: "speed",
    label: "SPEED",
    live: (context) => context.telemetry.speedKph,
    sample: (sample) => sample.speedKph,
    format: (value, context) => formatSpeed(value, context.profile.distanceUnit),
  },
  {
    metric: "wattsPerKilogram",
    label: "W/KG",
    // Rider weight alone: the bike is not what the rider is lifting up a climb.
    live: (context) => context.displayPowerWatts / context.profile.riderWeightKg,
    sample: (sample, context) => sample.powerWatts / context.profile.riderWeightKg,
    format: (value) => ({ value: value.toFixed(2), unit: "W/kg" }),
  },
];

const sampledDefinition = (
  metric: SampledMetric,
  scope: RideScope,
  aggregate: RideAggregate,
): Definition => ({
  label: `${scopeLabel(scope)}${aggregateLabel(aggregate)}${metric.label}`,
  resolve: (context) => {
    const value =
      aggregate === "current"
        ? metric.live(context)
        : aggregate === "average"
          ? timeWeightedMean(scopedHistory(scope, context), (sample) =>
              metric.sample(sample, context),
            )
          : peak(scopedHistory(scope, context), (sample) => metric.sample(sample, context));
    // A metric with no device (or no history yet) still holds its place.
    if (value === null || !Number.isFinite(value)) {
      return { value: MISSING, unit: metric.format(0, context).unit };
    }
    return metric.format(value, context);
  },
});

const energyDefinition = (scope: RideScope, calories: boolean): Definition => ({
  label: `${scopeLabel(scope)}${calories ? "CALORIES" : "ENERGY"}`,
  resolve: (context) => {
    const kilojoules = workKilojoules(scopedHistory(scope, context));
    return calories
      ? { value: String(Math.round(kilojoulesToKilocalories(kilojoules))), unit: "Cal" }
      : { value: String(Math.round(kilojoules)), unit: "kJ" };
  },
});

const timeDefinition = (scope: RideScope, aggregate: RideAggregate): Definition => {
  const remaining = aggregate === "remaining";
  return {
    label: scope === "interval" ? (remaining ? "BLOCK LEFT" : "BLOCK TIME") : remaining ? "REMAINING" : "ELAPSED",
    resolve: (context) => {
      const elapsed = scope === "interval" ? context.intervalElapsedSeconds : context.elapsedSeconds;
      if (!remaining) return { value: formatDuration(elapsed) };
      const total = scope === "interval" ? context.intervalTotalSeconds : context.totalSeconds;
      // An open-ended ride (or a free-ride block) has no time left in it, so
      // the slot is dropped rather than shown empty.
      return total === null ? null : { value: formatDuration(Math.max(0, total - elapsed)) };
    },
  };
};

const definitions = new Map<string, Definition>();

const define = (field: RideField, definition: Definition) => {
  definitions.set(rideFieldKey(field), definition);
};

for (const metric of sampledMetrics) {
  // W/kg has no meaningful peak: a single spike says nothing about the rider.
  const aggregates: RideAggregate[] =
    metric.metric === "wattsPerKilogram"
      ? ["current", "average"]
      : ["current", "average", "max"];
  for (const scope of ["ride", "interval"] as const) {
    for (const aggregate of aggregates) {
      // A live reading is the same number whatever the scope, so it is offered
      // once, on the ride.
      if (scope === "interval" && aggregate === "current") continue;
      define({ metric: metric.metric, scope, aggregate }, sampledDefinition(metric, scope, aggregate));
    }
  }
}

for (const scope of ["ride", "interval"] as const) {
  define({ metric: "energy", scope, aggregate: "total" }, energyDefinition(scope, false));
  define({ metric: "calories", scope, aggregate: "total" }, energyDefinition(scope, true));
  define({ metric: "time", scope, aggregate: "total" }, timeDefinition(scope, "total"));
  define({ metric: "time", scope, aggregate: "remaining" }, timeDefinition(scope, "remaining"));
}

// Distance is estimated by the runner over the whole ride; it does not keep a
// per-block total, so there is no block-scoped distance to offer.
define(
  { metric: "distance", scope: "ride", aggregate: "total" },
  {
    label: "DISTANCE",
    resolve: (context) => formatDistance(context.distanceMeters, context.profile.distanceUnit),
  },
);

define(
  { metric: "targetPower", scope: "ride", aggregate: "current" },
  {
    label: "TARGET",
    resolve: (context) =>
      context.targetPowerWatts === null
        ? { value: "Free" }
        : { value: String(context.targetPowerWatts), unit: "W" },
  },
);

/** Every field a rider can choose, in the order the editor offers them. */
export const rideFields: RideField[] = [...definitions.keys()].map((key) => {
  const [metric, scope, aggregate] = key.split(":");
  return {
    metric: metric as RideMetric,
    scope: scope as RideScope,
    aggregate: aggregate as RideAggregate,
  };
});

/** Editor-facing names. The tile headings are upper-case; these are not. */
export const rideMetricLabels: Record<RideMetric, string> = {
  power: "Power",
  cadence: "Cadence",
  heartRate: "Heart rate",
  speed: "Speed",
  wattsPerKilogram: "Watts per kilogram",
  distance: "Distance",
  energy: "Energy",
  calories: "Calories",
  time: "Time",
  targetPower: "Target power",
};

export const rideScopeLabels: Record<RideScope, string> = {
  ride: "Whole ride",
  interval: "Current block",
};

export const rideAggregateLabels: Record<RideAggregate, string> = {
  current: "Now",
  average: "Average",
  max: "Maximum",
  total: "Total",
  remaining: "Remaining",
};

/** Metrics that have at least one field, in the order the editor offers them. */
export const rideMetrics: RideMetric[] = [
  ...new Set(rideFields.map((field) => field.metric)),
];

export const rideScopesFor = (metric: RideMetric): RideScope[] => [
  ...new Set(rideFields.filter((field) => field.metric === metric).map((field) => field.scope)),
];

export const rideAggregatesFor = (metric: RideMetric, scope: RideScope): RideAggregate[] =>
  rideFields
    .filter((field) => field.metric === metric && field.scope === scope)
    .map((field) => field.aggregate);

/** The first field this metric offers, for when the editor switches metric. */
export const firstRideField = (metric: RideMetric): RideField =>
  rideFields.find((field) => field.metric === metric) ?? rideFields[0];

export const isRideField = (field: RideField): boolean =>
  definitions.has(rideFieldKey(field));

/** The tile heading for a field, or null if the combination does not exist. */
export const rideFieldLabel = (field: RideField): string | null =>
  definitions.get(rideFieldKey(field))?.label ?? null;

/**
 * The value to show for a field, or null when the field has nothing to say on
 * this ride (time remaining on a free ride). A null field is left out of the
 * screen rather than rendered empty.
 */
export const resolveRideField = (
  field: RideField,
  context: RideFieldContext,
): RideFieldValue | null => {
  const definition = definitions.get(rideFieldKey(field));
  if (definition === undefined) return null;
  const resolved = definition.resolve(context);
  return resolved === null ? null : { label: definition.label, ...resolved };
};
