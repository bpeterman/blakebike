import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import type {
  ConnectProgress,
  DeviceInfo,
  DeviceLogLine,
  DeviceRole,
  DeviceSlot,
  DeviceState,
  DevicesSnapshot,
  KnownDevice,
  PowerSmoothing,
  SourcePreferences,
  Profile,
  RideDisplayPreferences,
  RunnerState,
  SessionDetail,
  SessionSummary,
  Telemetry,
  TrainingZoneSettings,
  Workout,
} from "./types";

export type TrainingSyncResult = {
  profile: Profile;
  zones: TrainingZoneSettings;
  powerZonesImported: boolean;
  heartRateZonesImported: boolean;
};

export const api = {
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
  deviceLog: (role: DeviceRole) => invoke<DeviceLogLine[]>("device_log", { role }),
  knownDevices: () => invoke<KnownDevice[]>("known_devices"),
  forgetDevice: (id: string) => invoke<void>("forget_device", { id }),
  forgetAllDevices: () => invoke<number>("forget_all_devices"),
  sourcePreferences: () => invoke<SourcePreferences>("get_source_preferences"),
  saveSourcePreferences: (preferences: SourcePreferences) =>
    invoke<void>("set_source_preferences", { preferences }),
  profile: () => invoke<Profile>("get_profile"),
  saveProfile: (profile: Profile) =>
    invoke<void>("save_profile", { profile }),
  intervalsApiKeyConfigured: () =>
    invoke<boolean>("intervals_api_key_configured"),
  saveIntervalsApiKey: (apiKey: string) =>
    invoke<void>("save_intervals_api_key", { apiKey }),
  clearIntervalsApiKey: () =>
    invoke<void>("clear_intervals_api_key"),
  refreshEstimatedFtp: () =>
    invoke<TrainingSyncResult>("refresh_estimated_ftp"),
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
    }
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
    }
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
    }
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
