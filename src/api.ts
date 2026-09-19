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
  Profile,
  RunnerState,
  SessionDetail,
  SessionSummary,
  Telemetry,
  Workout,
} from "./types";

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
  profile: () => invoke<Profile>("get_profile"),
  saveProfile: (profile: Profile) =>
    invoke<void>("save_profile", { profile }),
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
