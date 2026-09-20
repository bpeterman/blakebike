import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import type { PowerComparison } from "./dualPower";
import type { RideDisplayPreferences } from "./rideScreens";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import type {
  CalibrationProgress,
  CalibrationRecord,
  ConnectProgress,
  DeviceInfo,
  DeviceLogLine,
  DeviceRole,
  DeviceSlot,
  DeviceState,
  DevicesSnapshot,
  IntervalsStatus,
  IntervalsSyncReport,
  IntervalsSyncSettings,
  KnownConnectOutcome,
  KnownDevice,
  PlannedWorkout,
  PowerSmoothing,
  SourcePreferences,
  Profile,
  RunnerState,
  SessionDetail,
  SessionSummary,
  Telemetry,
  TrainingSyncResult,
  TrainingZoneSettings,
  Workout,
} from "./types";

export type WorkoutExportResult = {
  exportedCount: number;
  directory: string;
};

export const websiteUrl = "https://blake.bike";

/** Header carrying the percent-encoded destination of a raw-body PNG export (mirrors Rust). */
const EXPORT_PATH_HEADER = "x-export-path";

export const api = {
  appVersion: () => getVersion(),
  openWebsite: () => openUrl(websiteUrl),
  deviceState: () => invoke<DeviceState>("device_state"),
  scanTrainers: () => invoke<DeviceInfo[]>("scan_trainers"),
  connectTrainer: (device: DeviceInfo) =>
    invoke<void>("connect_trainer", { device }),
  disconnectTrainer: () => invoke<void>("disconnect_trainer"),
  devicesSnapshot: () => invoke<DevicesSnapshot>("devices_snapshot"),
  scanDevices: () => invoke<DeviceInfo[]>("scan_devices"),
  connectDevice: (role: DeviceRole, device: DeviceInfo) =>
    invoke<void>("connect_device", { role, device }),
  disconnectDevice: (role: DeviceRole) =>
    invoke<void>("disconnect_device", { role }),
  calibrateDevice: (role: DeviceRole) =>
    invoke<CalibrationRecord>("calibrate_device", { role }),
  cancelCalibration: (role: DeviceRole) =>
    invoke<void>("cancel_calibration", { role }),
  deviceLog: (role: DeviceRole) => invoke<DeviceLogLine[]>("device_log", { role }),
  knownDevices: () => invoke<KnownDevice[]>("known_devices"),
  connectKnownDevices: () => invoke<KnownConnectOutcome[]>("connect_known_devices"),
  forgetDevice: (id: string) => invoke<void>("forget_device", { id }),
  forgetAllDevices: () => invoke<number>("forget_all_devices"),
  restoreKnownDevices: (devices: KnownDevice[]) =>
    invoke<void>("restore_known_devices", { devices }),
  sourcePreferences: () => invoke<SourcePreferences>("get_source_preferences"),
  saveSourcePreferences: (preferences: SourcePreferences) =>
    invoke<void>("set_source_preferences", { preferences }),
  profile: () => invoke<Profile>("get_profile"),
  saveProfile: (profile: Profile) =>
    invoke<void>("save_profile", { profile }),
  intervalsStatus: () => invoke<IntervalsStatus>("intervals_status"),
  saveIntervalsApiKey: (apiKey: string) =>
    invoke<IntervalsStatus>("save_intervals_api_key", { apiKey }),
  clearIntervalsApiKey: () =>
    invoke<IntervalsStatus>("clear_intervals_api_key"),
  saveIntervalsSyncSettings: (settings: IntervalsSyncSettings) =>
    invoke<IntervalsStatus>("set_intervals_sync_settings", { settings }),
  syncIntervals: () => invoke<IntervalsSyncReport>("sync_intervals"),
  plannedWorkouts: () => invoke<PlannedWorkout[]>("planned_workouts"),
  syncTrainingSettings: () =>
    invoke<TrainingSyncResult>("sync_training_settings"),
  workouts: () => invoke<Workout[]>("list_workouts"),
  workout: (id: string) => invoke<Workout | null>("get_workout", { id }),
  saveWorkout: (workout: Workout) =>
    invoke<Workout>("save_workout", { workout }),
  deleteWorkout: (id: string) => invoke<void>("delete_workout", { id }),
  importZwo: (contents: string) =>
    invoke<Workout>("import_zwo_workout", { contents }),
  exportZwo: async (workout: Workout) => {
    const path = await save({
      defaultPath: `${safeName(workout.name)}.zwo`,
      filters: [{ name: "Zwift workout", extensions: ["zwo"] }],
    });
    if (path) {
      await invoke<void>("export_zwo_workout", {
        workoutId: workout.id,
        path,
      });
      return true;
    }
    return false;
  },
  exportAllZwo: async (): Promise<WorkoutExportResult | null> => {
    const directory = await open({
      directory: true,
      multiple: false,
      title: "Export workout library",
    });
    if (typeof directory !== "string") return null;
    return invoke<WorkoutExportResult>("export_all_zwo_workouts", {
      directory,
    });
  },
  runnerState: () => invoke<RunnerState>("runner_state"),
  startWorkout: (workoutId: string) =>
    invoke<string>("start_workout", { workoutId }),
  startFreeRide: () => invoke<string>("start_free_ride"),
  adjustManualPower: (delta: number) =>
    invoke<number>("adjust_manual_power", { delta }),
  setManualPower: (watts: number) =>
    invoke<number>("set_manual_power", { watts }),
  clearTargetOverride: () => invoke<number | null>("clear_target_override"),
  setBiasPercent: (percent: number) =>
    invoke<number>("set_bias_percent", { percent }),
  powerSmoothing: () => invoke<PowerSmoothing>("get_power_smoothing"),
  savePowerSmoothing: (smoothing: PowerSmoothing) =>
    invoke<void>("set_power_smoothing", { smoothing }),
  devMode: () => invoke<boolean>("get_dev_mode"),
  saveDevMode: (enabled: boolean) => invoke<void>("set_dev_mode", { enabled }),
  trainingZones: () => invoke<TrainingZoneSettings>("get_training_zones"),
  saveTrainingZones: (zones: TrainingZoneSettings) =>
    invoke<void>("set_training_zones", { zones }),
  rideDisplayPreferences: () =>
    invoke<RideDisplayPreferences>("get_ride_display_preferences"),
  saveRideDisplayPreferences: (preferences: RideDisplayPreferences) =>
    invoke<void>("set_ride_display_preferences", { preferences }),
  pauseOrResume: () => invoke<void>("pause_or_resume_workout"),
  skipInterval: () => invoke<void>("skip_interval"),
  stopWorkout: () => invoke<void>("stop_workout"),
  sessions: () => invoke<SessionSummary[]>("list_sessions"),
  session: (id: string) =>
    invoke<SessionDetail | null>("get_session", { id }),
  /** Null for rides that did not record both power devices. */
  powerComparison: (id: string) =>
    invoke<PowerComparison | null>("get_power_comparison", { id }),
  exportPowerComparisonPng: async (session: SessionSummary, png: Uint8Array) => {
    const path = await save({
      defaultPath: `${safeName(session.workoutName)}-${session.startedAt.slice(0, 10)}-power-comparison.png`,
      filters: [{ name: "PNG image", extensions: ["png"] }],
    });
    if (!path) return false;
    // The image travels as the raw request body, not JSON.
    await invoke<void>("export_png", png, {
      headers: { [EXPORT_PATH_HEADER]: encodeURIComponent(path) },
    });
    return true;
  },
  exportSessionCsv: async (session: SessionSummary) => {
    const path = await save({
      defaultPath: `${safeName(session.workoutName)}-${session.startedAt.slice(0, 10)}.csv`,
      filters: [{ name: "CSV data", extensions: ["csv"] }],
    });
    if (path) {
      await invoke<void>("export_session_csv", {
        sessionId: session.id,
        path,
      });
      return true;
    }
    return false;
  },
  exportSessionFit: async (session: SessionSummary) => {
    const path = await save({
      defaultPath: `${safeName(session.workoutName)}-${session.startedAt.slice(0, 10)}.fit`,
      filters: [{ name: "Garmin FIT activity", extensions: ["fit"] }],
    });
    if (path) {
      await invoke<void>("export_session_fit", {
        sessionId: session.id,
        path,
      });
      return true;
    }
    return false;
  },
  prepareGarminUpload: (sessionId: string) =>
    invoke<string>("prepare_garmin_upload", { sessionId }),
  rideFilesPath: () => invoke<string>("get_ride_files_path"),
  revealRideFiles: () => invoke<void>("reveal_ride_files"),
  logFilePath: () => invoke<string>("get_log_file_path"),
  revealLogFile: () => invoke<void>("reveal_log_file"),
  reportError: (context: string, message: string) =>
    invoke<void>("report_client_error", { context, message }),
  reportEvent: (context: string, message: string) =>
    invoke<void>("report_client_event", { context, message }),
  onTelemetry: (handler: (telemetry: Telemetry) => void): Promise<UnlistenFn> =>
    listen<Telemetry>("trainer://telemetry", ({ payload }) => handler(payload)),
  onConnectProgress: (handler: (progress: ConnectProgress) => void): Promise<UnlistenFn> =>
    listen<ConnectProgress>("trainer://connect-progress", ({ payload }) => handler(payload)),
  onCalibrationProgress: (handler: (progress: CalibrationProgress) => void): Promise<UnlistenFn> =>
    listen<CalibrationProgress>("devices://calibration", ({ payload }) => handler(payload)),
  onDeviceSlot: (handler: (slot: DeviceSlot) => void): Promise<UnlistenFn> =>
    listen<DeviceSlot>("devices://slot", ({ payload }) => handler(payload)),
  onDeviceLog: (handler: (event: { role: DeviceRole; line: DeviceLogLine }) => void): Promise<UnlistenFn> =>
    listen<{ role: DeviceRole; line: DeviceLogLine }>("devices://log", ({ payload }) => handler(payload)),
  onRunnerState: (handler: (state: RunnerState) => void): Promise<UnlistenFn> =>
    listen<RunnerState>("workout://state", ({ payload }) => handler(payload)),
};

const safeName = (name: string) =>
  name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
