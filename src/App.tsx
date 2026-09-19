import {
  type ReactNode,
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  Activity,
  Bike,
  Bluetooth,
  ChevronDown,
  ChevronRight,
  ChevronUp,
  CircleStop,
  Download,
  ExternalLink,
  Gauge,
  HeartPulse,
  History,
  Info,
  Library,
  Pause,
  Play,
  Plus,
  RefreshCw,
  Settings,
  SkipForward,
  Trash2,
  Upload,
  X,
  Zap,
} from "lucide-react";
import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  CartesianGrid,
  Cell,
  ReferenceArea,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { api } from "./api";
import {
  type ChartRange,
  type RangeSummary,
  summarizeRange,
  useChartRangeDrag,
} from "./chartRange";
import {
  formatReleaseDate,
  releaseFor,
  releases,
  type Release,
} from "./releaseNotes";
import type {
  DeviceRole,
  DeviceState,
  DevicesSnapshot,
  Metric as SourceMetric,
  PowerSmoothing,
  Profile,
  RideCardId,
  RideDisplayPreferences,
  RunnerState,
  SessionDetail,
  SessionSummary,
  SourceChoice,
  SourcePreferences,
  Telemetry,
  TrainingZoneSettings,
  Workout,
  WorkoutInterval,
  WorkoutStep,
  ZoneDefinition,
} from "./types";
import {
  BIAS_STEP_PERCENT,
  clampBias,
  compileWorkoutIntervals,
  defaultRideDisplayPreferences,
  defaultTrainingZoneSettings,
  deviceRoleLabel,
  deviceRoles,
  formatDistance,
  formatDuration,
  formatIntervalTarget,
  formatSpeed,
  normalizeRideDisplayPreferences,
  downsampleTelemetry,
  effectiveHeartRateZones,
  effectivePowerZones,
  powerSmoothingLabel,
  powerSmoothingOptions,
  rideKeyAction,
  timeInZones,
  withActiveElapsed,
  withSmoothedPower,
  workoutDuration,
} from "./types";
import { DevicePicker } from "./DevicePicker";
import { DevicesPage } from "./DevicesPage";
import { TrainerCalibrationModal } from "./TrainerCalibrationModal";
import { isConnected as slotConnected, sourceNote } from "./devices";
import { DeviceStatsCard } from "./DeviceStats";
import { SourceSelect } from "./SourceSelect";
import { defaultSourcePreferences, withSourcePreference } from "./sourcePreferences";
import { shouldPromptForPostRide } from "./postRide";
import { ConfirmDialog } from "./ConfirmDialog";
import { EditableNumberInput } from "./EditableNumberInput";
import { Metric } from "./Metric";
import { WorkoutEditor } from "./WorkoutEditor";
import { WorkoutProfile } from "./WorkoutProfile";
import { useDialog } from "./useDialog";
import { zoneColorAt } from "./zones";
import "./App.css";

type Page = "home" | "workouts" | "devices" | "ride" | "history" | "settings";

type PostRidePromptState = {
  sessionId: string;
  errorMessage: string | null;
  saveWarning: string | null;
};

type UndoToastState = { message: string; action: () => Promise<void> };
type FeedbackState = { kind: "working" | "success"; message: string };

const LOG_LINES_KEPT = 200;

const emptyTelemetry: Telemetry = {
  timestampMs: 0,
  powerWatts: 0,
  cadenceRpm: null,
  speedKph: null,
  heartRateBpm: null,
  targetPowerWatts: null,
};

function App() {
  const [page, setPage] = useState<Page>("home");
  const [profile, setProfile] = useState<Profile | null>(null);
  const [workouts, setWorkouts] = useState<Workout[]>([]);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [hub, setHub] = useState<DevicesSnapshot | null>(null);
  const [runner, setRunner] = useState<RunnerState>({ status: "idle" });
  const [telemetry, setTelemetry] = useState(emptyTelemetry);
  const [telemetryHistory, setTelemetryHistory] = useState<Telemetry[]>([]);
  const [powerSmoothing, setPowerSmoothing] = useState<PowerSmoothing>("instant");
  const [trainingZones, setTrainingZones] = useState<TrainingZoneSettings>(
    defaultTrainingZoneSettings,
  );
  const powerZones = useMemo(
    () => (profile ? effectivePowerZones(trainingZones, profile.ftpWatts) : []),
    [profile, trainingZones],
  );
  const [rideDisplayPreferences, setRideDisplayPreferences] =
    useState<RideDisplayPreferences>(defaultRideDisplayPreferences);
  const [devMode, setDevMode] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [devicePicker, setDevicePicker] = useState<DeviceRole | null>(null);
  const [calibrationOpen, setCalibrationOpen] = useState(false);
  const [editor, setEditor] = useState<Workout | null>(null);
  const [selectedWorkout, setSelectedWorkout] = useState<string | null>(null);
  const [selectedSession, setSelectedSession] = useState<SessionDetail | null>(
    null,
  );
  const [postRidePrompt, setPostRidePrompt] =
    useState<PostRidePromptState | null>(null);
  const [feedback, setFeedback] = useState<FeedbackState | null>(null);
  const [undoToast, setUndoToast] = useState<UndoToastState | null>(null);
  const [endRideConfirm, setEndRideConfirm] = useState(false);
  const [endingRide, setEndingRide] = useState(false);
  const importRef = useRef<HTMLInputElement>(null);
  const runnerRef = useRef<RunnerState>(runner);
  const liveSessionRef = useRef<string | null>(null);
  const lastHistorySampleMsRef = useRef(0);

  const load = useCallback(async () => {
    try {
      const [
        nextProfile,
        nextWorkouts,
        nextSessions,
        nextHub,
        nextRunner,
        nextSmoothing,
        nextZones,
        nextRideDisplayPreferences,
        nextDevMode,
      ] =
        await Promise.all([
          api.profile(),
          api.workouts(),
          api.sessions(),
          api.devicesSnapshot(),
          api.runnerState(),
          api.powerSmoothing(),
          api.trainingZones(),
          api.rideDisplayPreferences(),
          api.devMode(),
        ]);
      setProfile(nextProfile);
      setWorkouts(nextWorkouts);
      setSessions(nextSessions);
      setHub(nextHub);
      setRunner(nextRunner);
      runnerRef.current = nextRunner;
      setPowerSmoothing(nextSmoothing);
      setTrainingZones(nextZones);
      setRideDisplayPreferences(
        normalizeRideDisplayPreferences(nextRideDisplayPreferences),
      );
      setDevMode(nextDevMode);
      if (nextRunner.status === "running" || nextRunner.status === "paused") {
        liveSessionRef.current = nextRunner.sessionId;
        const session = await api.session(nextRunner.sessionId);
        setTelemetryHistory(session?.samples ?? []);
        lastHistorySampleMsRef.current =
          session?.samples[session.samples.length - 1]?.timestampMs ?? 0;
      }
      setSelectedWorkout((current) => current ?? nextWorkouts[0]?.id ?? null);
    } catch (cause) {
      const message = messageOf(cause);
      setError(message);
      void api.reportError("initialization", message).catch(() => undefined);
    }
  }, []);

  useEffect(() => {
    // Anything that escapes React (render errors, forgotten awaits) still
    // lands in the backend log file.
    const onError = (event: ErrorEvent) =>
      void api.reportError("window.onerror", `${event.message} @ ${event.filename}:${event.lineno}`).catch(() => undefined);
    const onRejection = (event: PromiseRejectionEvent) =>
      void api.reportError("unhandledrejection", messageOf(event.reason)).catch(() => undefined);
    window.addEventListener("error", onError);
    window.addEventListener("unhandledrejection", onRejection);
    return () => {
      window.removeEventListener("error", onError);
      window.removeEventListener("unhandledrejection", onRejection);
    };
  }, []);

  useEffect(() => {
    void api.reportEvent("navigate", page).catch(() => undefined);
  }, [page]);

  useEffect(() => {
    void load();
    // Subscriptions resolve asynchronously; if this effect is cleaned up
    // before one resolves (StrictMode, fast unmount), unsubscribe it at once
    // instead of leaking a duplicate handler.
    let cancelled = false;
    const subscriptions: Array<() => void> = [];
    const track = (subscription: Promise<() => void>) => {
      void subscription.then((off) => {
        if (cancelled) off();
        else subscriptions.push(off);
      });
    };
    track(api.onTelemetry((sample) => {
      setTelemetry(sample);
      if (
        runnerRef.current.status === "running" &&
        sample.timestampMs - lastHistorySampleMsRef.current >= 1_000
      ) {
        lastHistorySampleMsRef.current = sample.timestampMs;
        setTelemetryHistory((history) => [...history, sample]);
      }
    }));
    track(api.onRunnerState((state) => {
      const previousState = runnerRef.current;
      setRunner(state);
      runnerRef.current = state;
      if (
        state.status === "running" &&
        liveSessionRef.current !== state.sessionId
      ) {
        liveSessionRef.current = state.sessionId;
        lastHistorySampleMsRef.current = 0;
        setTelemetryHistory([]);
      }
      if (state.status === "finished" || state.status === "error") {
        void api.sessions().then(setSessions);
      }
      if (shouldPromptForPostRide(previousState, state)) {
        const sessionId = state.sessionId;
        const errorMessage = state.status === "error" ? state.message : null;
        const saveWarning = state.status === "finished" ? state.saveWarning ?? null : null;
        setPostRidePrompt(null);
        void api.session(sessionId).then((session) => {
          if (!session) {
            setPostRidePrompt({ sessionId, errorMessage, saveWarning });
            return;
          }
          setSelectedSession(session);
          setPage("history");
        }).catch(() => {
          // If the saved ride cannot be loaded immediately, retain the
          // existing recovery prompt so the rider can try again.
          setPostRidePrompt({ sessionId, errorMessage, saveWarning });
        });
      }
    }));
    // Devices hub: whole-slot updates on state/stats changes, plus individual
    // log lines so the per-device logs grow live between snapshots.
    track(api.onDeviceSlot((slot) => {
      setHub((current) =>
        current
          ? { ...current, slots: current.slots.map((existing) => (existing.role === slot.role ? slot : existing)) }
          : current,
      );
    }));
    track(api.onDeviceLog(({ role, line }) => {
      setHub((current) =>
        current
          ? {
              ...current,
              slots: current.slots.map((existing) =>
                existing.role === role
                  ? { ...existing, log: [...existing.log.slice(-(LOG_LINES_KEPT - 1)), line] }
                  : existing,
              ),
            }
          : current,
      );
    }));
    return () => {
      cancelled = true;
      subscriptions.forEach((off) => off());
    };
  }, [load]);

  const deviceState: DeviceState =
    hub?.slots.find((slot) => slot.role === "trainer")?.state ?? { status: "idle" };
  const connected =
    deviceState.status === "ready" || deviceState.status === "controlling";
  const connectedRoles = deviceRoles.filter((role) =>
    slotConnected(hub?.slots.find((slot) => slot.role === role)?.state),
  );
  const active =
    runner.status === "running" ||
    runner.status === "paused";
  const riding = runner.status === "running" || runner.status === "paused";

  const perform = useCallback(async (action: () => Promise<unknown>, label = "user action") => {
    const visibleFeedback = !label.startsWith("load ");
    try {
      setError(null);
      setNotice(null);
      if (visibleFeedback) setFeedback({ kind: "working", message: workingCopy(label) });
      const result = await action();
      if (visibleFeedback) {
        setFeedback(result === false || result === null
          ? null
          : { kind: "success", message: successCopy(label) });
      }
    } catch (cause) {
      const message = messageOf(cause);
      setFeedback(null);
      setError(message);
      void api.reportError(label, message).catch(() => undefined);
    } finally {
      // Scan-level state (including the optional ANT adapter) is not tied to a
      // role slot event, so refresh the compact hub snapshot after actions.
      void api.devicesSnapshot().then(setHub).catch(() => undefined);
    }
  }, []);

  useEffect(() => {
    if (feedback?.kind !== "success") return;
    const timer = window.setTimeout(() => setFeedback(null), 2_600);
    return () => window.clearTimeout(timer);
  }, [feedback]);

  useEffect(() => {
    if (!undoToast) return;
    const timer = window.setTimeout(() => setUndoToast(null), 8_000);
    return () => window.clearTimeout(timer);
  }, [undoToast]);

  const offerUndo = useCallback((message: string, action: () => Promise<void>) => {
    setUndoToast({ message, action });
  }, []);

  const runUndo = async () => {
    const pending = undoToast;
    if (!pending) return;
    setUndoToast(null);
    await perform(pending.action, "restore removed item");
  };

  // The smoothing choice is remembered across launches; apply it at once and
  // save in the background so the toggle never feels laggy.
  const changePowerSmoothing = (smoothing: PowerSmoothing) => {
    setPowerSmoothing(smoothing);
    void api.savePowerSmoothing(smoothing).catch((cause) =>
      void api.reportError("save power smoothing", messageOf(cause)).catch(() => undefined),
    );
  };

  const changeSourcePreference = (metric: SourceMetric, choice: SourceChoice) => {
    const next = withSourcePreference(
      hub?.sourcePreferences ?? defaultSourcePreferences,
      metric,
      choice,
    );
    setHub((current) => current ? { ...current, sourcePreferences: next } : current);
    void perform(() => api.saveSourcePreferences(next), "save source preferences");
  };

  const openSession = useCallback(async (sessionId: string) => {
    await perform(async () => {
      const session = await api.session(sessionId);
      if (!session) throw new Error("The saved ride could not be found");
      setSelectedSession(session);
      setPage("history");
      setPostRidePrompt(null);
    }, "open session");
  }, [perform]);

  const openRide = (workoutId: string) => {
    setSelectedWorkout(workoutId);
    setPage("ride");
  };

  const importZwo = async (file: File) => {
    await perform(async () => {
      await api.importZwo(await file.text());
      setWorkouts(await api.workouts());
    }, "import zwo");
  };

  const navItems: Array<[Page, string, typeof Activity]> = [
    ["home", "Overview", Activity],
    ["workouts", "Workouts", Library],
    ["ride", "Ride", Bike],
    ["history", "History", History],
    ["devices", "Devices", Bluetooth],
    ["settings", "Settings", Settings],
  ];

  if (!profile) {
    return (
      <div className="loading">
        {error ? (
          <div className="loading-error">
            <h2>blake.bike could not start</h2>
            <p>{error}</p>
            <button type="button" onClick={() => { setError(null); void load(); }}>
              Try again
            </button>
          </div>
        ) : (
          "Preparing your training space…"
        )}
      </div>
    );
  }

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <img className="brand-mark" src="/blakebike-icon.svg" alt="" />
          <span>blake.bike</span>
        </div>
        <nav>
          {navItems.map(([id, label, Icon]) => (
            <button
              className={page === id ? "nav-item active" : "nav-item"}
              key={id}
              onClick={() => setPage(id)}
            >
              <Icon size={19} />
              <span>{label}</span>
              {id === "ride" && active && <i className="live-dot" />}
            </button>
          ))}
        </nav>
        <div className="sidebar-bottom">
          {riding && (
            <div className="sidebar-ride-controls">
              <span className="label">RIDE CONTROLS</span>
              <div>
                <button className="secondary" onClick={() => void perform(() => api.pauseOrResume(), "pause/resume")}>
                  {runner.status === "paused" ? <Play /> : <Pause />}
                  {runner.status === "paused" ? "Resume" : "Pause"}
                </button>
                <button className="stop" onClick={() => setEndRideConfirm(true)}>
                  <CircleStop /> End
                </button>
              </div>
            </div>
          )}
          <button className="connection-pill" onClick={() => setPage("devices")}>
            <span className="role-dots">
              {deviceRoles.map((role) => {
                const state = hub?.slots.find((slot) => slot.role === role)?.state;
                const tone =
                  slotConnected(state) ? "online"
                  : state?.status === "reconnecting" || state?.status === "error" ? "error"
                  : state?.status === "connecting" ? "busy"
                  : "";
                return <span key={role} className={`status-dot ${tone}`} title={deviceRoleLabel[role]} />;
              })}
            </span>
            <span>
              <small>DEVICES</small>
              <strong>
                {connectedRoles.length === 0
                  ? "None connected"
                  : connected && connectedRoles.length === 1
                    ? deviceState.device.name
                    : `${connectedRoles.length} of ${deviceRoles.length} connected`}
              </strong>
            </span>
            <ChevronRight size={16} />
          </button>
        </div>
      </aside>

      <main className="content">
        {page === "home" && (
          <Overview
            profile={profile}
            powerZones={powerZones}
            workouts={workouts}
            sessions={sessions}
            connected={connected}
            devMode={devMode}
            onConnect={() => setDevicePicker("trainer")}
            onRide={openRide}
            onNavigate={setPage}
            onRefreshFtp={() =>
              perform(async () => {
                const result = await api.refreshEstimatedFtp();
                setProfile(result.profile);
                setTrainingZones(result.zones);
              }, "refresh training settings")
            }
          />
        )}
        {page === "devices" && (
          <DevicesPage
            hub={hub}
            sources={telemetry.sources}
            onConnect={setDevicePicker}
            onCalibrate={() => setCalibrationOpen(true)}
            onSourcePreference={changeSourcePreference}
            onNotice={setNotice}
            onOfferUndo={offerUndo}
            perform={perform}
          />
        )}
        {page === "workouts" && (
          <WorkoutLibrary
            workouts={workouts}
            ftp={profile.ftpWatts}
            powerZones={powerZones}
            onCreate={() => setEditor(newWorkout())}
            onEdit={setEditor}
            onRide={openRide}
            onDelete={(workout) =>
              void perform(async () => {
                await api.deleteWorkout(workout.id);
                setWorkouts(await api.workouts());
                offerUndo(`“${workout.name}” deleted.`, async () => {
                  await api.saveWorkout(workout);
                  setWorkouts(await api.workouts());
                });
              }, "delete workout")
            }
            onExport={(workout) => void perform(() => api.exportZwo(workout), "export zwo")}
            onExportAll={() =>
              void perform(async () => {
                const result = await api.exportAllZwo();
                if (result) {
                  const label = result.exportedCount === 1 ? "workout" : "workouts";
                  setNotice(`Exported ${result.exportedCount} ${label} to ${result.directory}`);
                }
                return false;
              }, "export workout library")
            }
            onImport={() => importRef.current?.click()}
          />
        )}
        {page === "ride" && (
          <Ride
            workouts={workouts}
            selectedWorkout={selectedWorkout}
            setSelectedWorkout={setSelectedWorkout}
            connected={connected}
            onConnect={() => setDevicePicker("trainer")}
            runner={runner}
            telemetry={telemetry}
            telemetryHistory={telemetryHistory}
            powerSmoothing={powerSmoothing}
            onPowerSmoothing={changePowerSmoothing}
            sourcePreferences={hub?.sourcePreferences ?? defaultSourcePreferences}
            onSourcePreference={changeSourcePreference}
            hub={hub}
            profile={profile}
            trainingZones={trainingZones}
            displayPreferences={rideDisplayPreferences}
            onBrowseWorkouts={() => setPage("workouts")}
            perform={perform}
          />
        )}
        {page === "history" && (
          <HistoryPage
            sessions={sessions}
            selected={selectedSession}
            profile={profile}
            trainingZones={trainingZones}
            onSelect={(session) =>
              void openSession(session.id)
            }
            onClose={() => setSelectedSession(null)}
            onExport={(session) =>
              void perform(() => api.exportSessionCsv(session), "export csv")
            }
            onExportFit={(session) =>
              void perform(() => api.exportSessionFit(session), "export fit")
            }
            onGarmin={(session) =>
              void perform(() => api.prepareGarminUpload(session.id), "prepare Garmin upload")
            }
          />
        )}
        {page === "settings" && (
          <SettingsPage
            profile={profile}
            trainingZones={trainingZones}
            rideDisplayPreferences={rideDisplayPreferences}
            devMode={devMode}
            perform={perform}
            onProfileUpdate={setProfile}
            onTrainingZonesUpdate={setTrainingZones}
            onSave={(next) =>
              void perform(async () => {
                await api.saveProfile(next);
                setProfile(next);
              }, "save profile")
            }
            onSaveTrainingZones={(zones) =>
              void perform(async () => {
                await api.saveTrainingZones(zones);
                setTrainingZones(zones);
              }, "save training zones")
            }
            onSaveRideDisplayPreferences={(preferences) =>
              void perform(async () => {
                const normalized = normalizeRideDisplayPreferences(preferences);
                await api.saveRideDisplayPreferences(normalized);
                setRideDisplayPreferences(normalized);
              }, "save ride layout")
            }
            onForgetDevices={() => perform(async () => {
              const remembered = await api.knownDevices();
              await api.forgetAllDevices();
              if (remembered.length > 0) {
                offerUndo(`${remembered.length} remembered ${remembered.length === 1 ? "device" : "devices"} removed.`, () => api.restoreKnownDevices(remembered));
              }
            }, "forget all devices")}
            onDevMode={(enabled) =>
              void perform(async () => {
                await api.saveDevMode(enabled);
                setDevMode(enabled);
              }, "save developer mode")
            }
          />
        )}
        {page !== "ride" && riding && runner.recordingWarning && <RecordingWarning message={runner.recordingWarning} />}
      </main>

      <div className="toast-stack" aria-live="polite">
      {error && (
        <div className="toast error-toast" role="alert">
          <span>{error}</span>
          <button onClick={() => setError(null)} aria-label="Dismiss error"><X size={17} /></button>
        </div>
      )}
      {notice && (
        <div className="toast success-toast" role="status">
          <span>{notice}</span>
          <button onClick={() => setNotice(null)} aria-label="Dismiss notification"><X size={17} /></button>
        </div>
      )}
      {postRidePrompt && (
        <PostRidePrompt
          errorMessage={postRidePrompt.errorMessage}
          saveWarning={postRidePrompt.saveWarning}
          onView={() => void openSession(postRidePrompt.sessionId)}
          onDismiss={() => setPostRidePrompt(null)}
        />
      )}
      {feedback && (
        <div className={`toast action-toast ${feedback.kind === "working" ? "working-toast" : "success-toast"}`} role="status">
          {feedback.kind === "working" ? <span className="mini-spinner" aria-hidden="true" /> : <span className="success-mark" aria-hidden="true">✓</span>}
          <span>{feedback.message}</span>
        </div>
      )}
      {undoToast && (
        <div className="toast undo-toast" role="status">
          <span>{undoToast.message}</span>
          <button className="undo-button" onClick={() => void runUndo()}>Undo</button>
          <button onClick={() => setUndoToast(null)} aria-label="Dismiss undo"><X size={17} /></button>
        </div>
      )}
      </div>
      {endRideConfirm && (
        <ConfirmDialog
          title="End this ride?"
          description="Your progress will be saved and the trainer target will be released. You can’t resume this ride afterward."
          confirmLabel="End and save ride"
          busyLabel="Ending ride…"
          busy={endingRide}
          onClose={() => setEndRideConfirm(false)}
          onConfirm={() => {
            setEndingRide(true);
            void perform(async () => { await api.stopWorkout(); return false; }, "stop workout").finally(() => {
              setEndingRide(false);
              setEndRideConfirm(false);
            });
          }}
        />
      )}
      {devicePicker && (
        <DevicePicker
          role={devicePicker}
          slot={hub?.slots.find((slot) => slot.role === devicePicker)}
          scanError={hub?.scanError ?? null}
          antAdapter={hub?.antAdapter}
          close={() => setDevicePicker(null)}
          perform={perform}
        />
      )}
      {calibrationOpen && (
        <TrainerCalibrationModal
          speedKph={telemetry.speedKph}
          close={() => setCalibrationOpen(false)}
        />
      )}
      {editor && (
        <WorkoutEditor
          initial={editor}
          ftpWatts={profile.ftpWatts}
          powerZones={powerZones}
          close={() => setEditor(null)}
          save={(workout) =>
            void perform(async () => {
              await api.saveWorkout(workout);
              setWorkouts(await api.workouts());
              setEditor(null);
            }, "save workout")
          }
        />
      )}
      <input
        ref={importRef}
        hidden
        type="file"
        accept=".zwo,application/xml,text/xml"
        onChange={(event) => {
          const file = event.target.files?.[0];
          if (file) void importZwo(file);
          event.target.value = "";
        }}
      />
    </div>
  );
}

export default App;

function PageHeader({
  eyebrow,
  title,
  actions,
}: {
  eyebrow: string;
  title: string;
  actions?: React.ReactNode;
}) {
  return (
    <header className="page-header">
      <div><span>{eyebrow}</span><h1>{title}</h1></div>
      <div className="header-actions">{actions}</div>
    </header>
  );
}

function Overview({
  profile,
  powerZones,
  workouts,
  sessions,
  connected,
  devMode,
  onConnect,
  onRide,
  onNavigate,
  onRefreshFtp,
}: {
  profile: Profile;
  powerZones: readonly ZoneDefinition[];
  workouts: Workout[];
  sessions: SessionSummary[];
  connected: boolean;
  devMode: boolean;
  onConnect: () => void;
  onRide: (id: string) => void;
  onNavigate: (page: Page) => void;
  onRefreshFtp: () => Promise<void>;
}) {
  const latest = sessions[0];
  const [refreshingFtp, setRefreshingFtp] = useState(false);
  const refreshFtp = async () => {
    setRefreshingFtp(true);
    try {
      await onRefreshFtp();
    } finally {
      setRefreshingFtp(false);
    }
  };
  return (
    <>
      <PageHeader eyebrow="GOOD EVENING" title={`Ready to ride, ${profile.name}?`} />
      <section className="hero-grid">
        <article className="card connection-card">
          <div className="card-icon"><Bluetooth /></div>
          <div><span className="label">SMART TRAINER</span><h2>{connected ? "Trainer ready" : "Connect your trainer"}</h2>
            <p>{connected ? "FTMS control is available. Choose a workout when you’re ready." : devMode ? "Pair an FTMS Bluetooth trainer or use the built-in simulator." : "Pair an FTMS Bluetooth trainer to get started."}</p>
          </div>
          <button className={connected ? "secondary" : "primary"} onClick={onConnect}>{connected ? "Manage" : "Connect"}</button>
        </article>
        <article className="card ftp-card">
          <button
            className="icon-button ftp-refresh"
            title="Refresh from Intervals.icu"
            aria-label="Refresh from Intervals.icu"
            disabled={refreshingFtp}
            onClick={() => void refreshFtp()}
          >
            <RefreshCw size={16} className={refreshingFtp ? "spinning" : ""} />
          </button>
          <span className="label">CURRENT FTP</span>
          <div className="big-number">{profile.ftpWatts}<small> W</small></div>
          <p>Power targets scale from your rider profile.</p>
          <button className="text-button" onClick={() => onNavigate("settings")}>Update profile <ChevronRight size={15} /></button>
        </article>
      </section>
      <section className="section-heading"><div><span className="label">UP NEXT</span><h2>Choose a workout</h2></div><button className="text-button" onClick={() => onNavigate("workouts")}>View library <ChevronRight size={15} /></button></section>
      <div className="workout-row">
        {workouts.slice(0, 3).map((workout, index) => (
          <article className="card workout-card" key={workout.id}>
            <div className={`workout-visual tone-${index % 3}`}><WorkoutProfile steps={workout.steps} ftpWatts={profile.ftpWatts} powerZones={powerZones} /></div>
            <span className="label">{formatDuration(workoutDuration(workout.steps))} · {workout.steps.length} BLOCKS</span>
            <h3>{workout.name}</h3><p>{workout.description || "A structured power workout."}</p>
            <button className="secondary" onClick={() => onRide(workout.id)}><Play size={16} fill="currentColor" /> Ride</button>
          </article>
        ))}
      </div>
      {latest && (
        <section className="recent-strip" onClick={() => onNavigate("history")}>
          <div className="card-icon subtle"><History /></div>
          <div><span className="label">LATEST RIDE</span><strong>{latest.workoutName}</strong><small>{new Date(latest.startedAt).toLocaleDateString()}</small></div>
          <Metric value={`${latest.averagePowerWatts}`} unit="W avg" />
          <Metric value={formatDuration(latest.elapsedSeconds)} unit="duration" />
          <ChevronRight />
        </section>
      )}
    </>
  );
}

export function WorkoutLibrary({
  workouts,
  ftp,
  powerZones,
  onCreate,
  onEdit,
  onRide,
  onDelete,
  onExport,
  onExportAll,
  onImport,
}: {
  workouts: Workout[];
  ftp: number;
  powerZones: readonly ZoneDefinition[];
  onCreate: () => void;
  onEdit: (workout: Workout) => void;
  onRide: (id: string) => void;
  onDelete: (workout: Workout) => void;
  onExport: (workout: Workout) => void;
  onExportAll: () => void;
  onImport: () => void;
}) {
  return (
    <>
      <PageHeader eyebrow={`${workouts.length} SAVED WORKOUTS`} title="Workout library" actions={<>
        <button className="secondary" onClick={onImport}><Upload size={16} /> Import ZWO</button>
        <button className="secondary" disabled={workouts.length === 0} onClick={onExportAll}><Download size={16} /> Export all ZWO</button>
        <button className="primary" onClick={onCreate}><Plus size={17} /> New workout</button>
      </>} />
      {workouts.length === 0 ? (
        <div className="empty-state card friendly-empty">
          <Library size={36} />
          <h2>Your next ride starts here</h2>
          <p>Create a workout from scratch or import a ZWO file you already love.</p>
          <div className="empty-actions">
            <button className="primary" onClick={onCreate}><Plus size={17} /> Create a workout</button>
            <button className="secondary" onClick={onImport}><Upload size={16} /> Import ZWO</button>
          </div>
        </div>
      ) : <div className="library-grid">
        {workouts.map((workout, index) => (
          <article className="card library-card" key={workout.id}>
            <button className="workout-visual-button" onClick={() => onEdit(workout)}>
              <div className={`workout-visual large tone-${index % 3}`}><WorkoutProfile steps={workout.steps} ftpWatts={ftp} powerZones={powerZones} /></div>
            </button>
            <div className="library-body">
              <span className="label">{workout.source.toUpperCase()} · {Math.round(workoutDuration(workout.steps) / 60)} MIN</span>
              <h3>{workout.name}</h3>
              <p>{workout.description || "Structured workout"}</p>
              <div className="chips"><span>{workout.steps.length} blocks</span><span>{ftp} W FTP</span></div>
              <div className="card-actions">
                <button className="primary" onClick={() => onRide(workout.id)}><Play size={15} fill="currentColor" /> Ride</button>
                <button className="icon-button" onClick={() => onExport(workout)} title="Export ZWO" aria-label={`Export ${workout.name} as ZWO`}><Download size={17} /></button>
                <button className="icon-button danger" onClick={() => onDelete(workout)} title="Delete" aria-label={`Delete ${workout.name}`}><Trash2 size={17} /></button>
              </div>
            </div>
          </article>
        ))}
      </div>}
    </>
  );
}

export function Ride({
  workouts,
  selectedWorkout,
  setSelectedWorkout,
  connected,
  onConnect,
  runner,
  telemetry,
  telemetryHistory,
  powerSmoothing,
  onPowerSmoothing,
  sourcePreferences,
  onSourcePreference,
  hub,
  profile,
  trainingZones,
  displayPreferences,
  onBrowseWorkouts = () => undefined,
  perform,
}: {
  workouts: Workout[];
  selectedWorkout: string | null;
  setSelectedWorkout: (id: string) => void;
  connected: boolean;
  onConnect: () => void;
  runner: RunnerState;
  telemetry: Telemetry;
  telemetryHistory: Telemetry[];
  powerSmoothing: PowerSmoothing;
  onPowerSmoothing: (smoothing: PowerSmoothing) => void;
  sourcePreferences: SourcePreferences;
  onSourcePreference: (metric: SourceMetric, choice: SourceChoice) => void;
  /** The devices hub, for the stats-for-nerds card. */
  hub: DevicesSnapshot | null;
  profile: Profile;
  trainingZones: TrainingZoneSettings;
  displayPreferences: RideDisplayPreferences;
  onBrowseWorkouts?: () => void;
  perform: (action: () => Promise<unknown>, label?: string) => Promise<void>;
}) {
  const [targetDraft, setTargetDraft] = useState("100");
  const [powerChartExpanded, setPowerChartExpanded] = useState(true);
  const [heartRateChartExpanded, setHeartRateChartExpanded] = useState(true);
  const active = runner.status === "running" || runner.status === "paused";
  const selected = workouts.find((workout) => workout.id === selectedWorkout);
  const activeWorkout = selected
    ?? ("workoutName" in runner
      ? workouts.find((workout) => workout.name === runner.workoutName)
      : undefined);
  const elapsed = runner.status === "running" || runner.status === "paused" ? runner.elapsedSeconds : 0;
  const total = runner.status === "running" || runner.status === "paused" ? runner.totalSeconds : selected ? workoutDuration(selected.steps) : 0;
  const progress = total ? Math.min(100, (elapsed / total) * 100) : 0;
  const riding = runner.status === "running" || runner.status === "paused";
  const manualErg = riding && runner.manualErg;
  const openEnded = riding && runner.totalSeconds === null;
  const targetPower = riding ? runner.targetPowerWatts : telemetry.targetPowerWatts;
  // Structured workouts get the same target controls as free ride: ± 5 W,
  // typed watts and ↑/↓ override the current interval; a bias scales the plan.
  const plannedTarget = riding && !runner.manualErg ? runner.plannedTargetWatts : null;
  const overrideActive = riding && !runner.manualErg && runner.overrideActive;
  const biasPercent = riding ? runner.biasPercent : 100;
  const structured = riding && !openEnded;
  const adjustable = manualErg || openEnded || (structured && targetPower !== null);
  const displayedSpeed = formatSpeed(telemetry.speedKph ?? 0, profile.distanceUnit);
  const smoothedHistory = useMemo(
    () => withSmoothedPower(telemetryHistory, powerSmoothing),
    [telemetryHistory, powerSmoothing],
  );
  const chartHistory = useMemo(
    () => downsampleTelemetry(withActiveElapsed(smoothedHistory, elapsed)),
    [elapsed, smoothedHistory],
  );
  const powerZones = useMemo(
    () => effectivePowerZones(trainingZones, profile.ftpWatts),
    [profile.ftpWatts, trainingZones],
  );
  const heartRateZones = useMemo(
    () => effectiveHeartRateZones(trainingZones, profile.maxHeartRateBpm),
    [profile.maxHeartRateBpm, trainingZones],
  );
  const powerZoneSeconds = useMemo(
    () => timeInZones(telemetryHistory, powerZones, "power"),
    [powerZones, telemetryHistory],
  );
  const heartRateZoneSeconds = useMemo(
    () => timeInZones(telemetryHistory, heartRateZones, "heartRate"),
    [heartRateZones, telemetryHistory],
  );
  const displayedPower =
    powerSmoothing === "instant"
      ? telemetry.powerWatts
      : smoothedHistory[smoothedHistory.length - 1]?.displayPowerWatts ?? telemetry.powerWatts;
  // Biased like the runner's targets, so the timeline shows what the trainer is asked for.
  const workoutIntervals = useMemo(
    () => activeWorkout
      ? compileWorkoutIntervals(activeWorkout.steps, profile.ftpWatts, biasPercent)
      : [],
    [activeWorkout, profile.ftpWatts, biasPercent],
  );

  useEffect(() => {
    if (targetPower !== null) setTargetDraft(String(targetPower));
  }, [targetPower]);

  const adjustPower = (delta: number) =>
    perform(async () => {
      setTargetDraft(String(await api.adjustManualPower(delta)));
    }, delta > 0 ? "raise manual power" : "lower manual power");

  const commitTargetPower = () => {
    const watts = Math.round(Number(targetDraft));
    if (!Number.isFinite(watts) || watts <= 0 || watts > 65_535) {
      setTargetDraft(String(targetPower ?? 100));
      return;
    }
    void perform(async () => {
      setTargetDraft(String(await api.setManualPower(watts)));
    }, "set manual power");
  };

  const adjustBias = (delta: number) =>
    perform(async () => {
      await api.setBiasPercent(clampBias(biasPercent + delta));
    }, delta > 0 ? "raise workout bias" : "lower workout bias");

  const backToPlan = () =>
    perform(async () => {
      const applied = await api.clearTargetOverride();
      if (applied !== null) setTargetDraft(String(applied));
    }, "clear target override");

  useEffect(() => {
    if (runner.status !== "running" || !adjustable) return;
    const onKeyDown = (event: KeyboardEvent) => {
      // Leave typing in the watts box alone.
      if (event.target instanceof HTMLInputElement) return;
      const action = rideKeyAction(event.key, event.shiftKey, event.repeat);
      if (action === null) return;
      if (action.kind === "bias" && !structured) return;
      event.preventDefault();
      if (action.kind === "power") {
        void perform(() => api.adjustManualPower(action.delta), "adjust manual power");
      } else {
        void perform(() => api.setBiasPercent(clampBias(biasPercent + action.delta)), "adjust workout bias");
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [adjustable, biasPercent, perform, runner.status, structured]);

  const cardContent = {
    power: (
      <div className="ride-card-compact">
        <LiveMetric icon={Zap} label="POWER" value={displayedPower} unit="W" accent note={sourceNote(telemetry.sources?.power)}
          control={
            <span className="metric-control-group">
              <SourceSelect
                metric="power"
                choice={sourcePreferences.power}
                onChange={(choice) => onSourcePreference("power", choice)}
              />
              <label className="smoothing-select">
                <span className="sr-only">Power smoothing</span>
                <select value={powerSmoothing} onChange={(event) => onPowerSmoothing(event.target.value as PowerSmoothing)}>
                  {powerSmoothingOptions.map((option) => (
                    <option key={option} value={option}>{powerSmoothingLabel[option]}</option>
                  ))}
                </select>
              </label>
            </span>
          } />
      </div>
    ),
    cadence: (
      <div className="ride-card-compact">
        <LiveMetric
          icon={RefreshCw}
          label="CADENCE"
          value={Math.round(telemetry.cadenceRpm ?? 0)}
          unit="rpm"
          note={sourceNote(telemetry.sources?.cadence)}
          control={
            <SourceSelect
              metric="cadence"
              choice={sourcePreferences.cadence}
              onChange={(choice) => onSourcePreference("cadence", choice)}
            />
          }
        />
      </div>
    ),
    speed: (
      <div className="ride-card-compact">
        <LiveMetric icon={Gauge} label="SPEED" value={displayedSpeed.value} unit={displayedSpeed.unit} />
      </div>
    ),
    heartRate: (
      <div className="ride-card-compact">
        <LiveMetric icon={HeartPulse} label="HEART RATE" value={telemetry.heartRateBpm ?? "—"} unit="bpm" note={sourceNote(telemetry.sources?.heartRate)} />
      </div>
    ),
    workoutTimeline: structured && activeWorkout && workoutIntervals.length > 0 ? (
      <WorkoutTimeline
        steps={activeWorkout.steps}
        ftpWatts={profile.ftpWatts}
        biasPercent={biasPercent}
        powerZones={powerZones}
        intervals={workoutIntervals}
        workoutName={runner.workoutName}
        currentIndex={runner.intervalIndex}
        intervalElapsedSeconds={runner.intervalElapsedSeconds}
        elapsedSeconds={elapsed}
        totalSeconds={total ?? 0}
        paused={runner.status === "paused"}
        onSkip={() => void perform(() => api.skipInterval(), "skip interval")}
      />
    ) : null,
    targetAndBias: (
      <div className="card workout-controls-card">
        <div className="workout-controls-heading">
          <span className="label">TARGET &amp; BIAS</span>
        </div>
        <div className={adjustable ? "target-line editable" : "target-line"}>
          <span>Target power</span>
          {adjustable ? (
            <div className="manual-erg-stepper">
              <button className="secondary" disabled={runner.status !== "running"} onClick={() => void adjustPower(-5)}>− 5 W</button>
              <label>
                <span className="sr-only">Target power in watts</span>
                <input
                  type="number"
                  min="1"
                  max="65535"
                  step="5"
                  disabled={runner.status !== "running"}
                  value={targetDraft}
                  onChange={(event) => setTargetDraft(event.target.value)}
                  onBlur={commitTargetPower}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") event.currentTarget.blur();
                  }}
                />
                <strong>W</strong>
              </label>
              <button className="secondary" disabled={runner.status !== "running"} onClick={() => void adjustPower(5)}>+ 5 W</button>
            </div>
          ) : <strong>{targetPower ?? "Free"}{targetPower !== null ? " W" : ""}</strong>}
          {adjustable && <span className="manual-erg-hint">{structured ? "Type watts or use ↑ / ↓ · Shift for bias" : "Type watts or use ↑ / ↓"}</span>}
        </div>
        {structured && (
          <div className="plan-line">
            <span className="plan-note">
              {manualErg
                ? "Free-ride block · manual ERG"
                : plannedTarget === null
                  ? "No target in this block"
                  : overrideActive
                    ? `Plan ${plannedTarget} W · overridden for this block`
                    : biasPercent !== 100
                      ? `Plan ${plannedTarget} W · ${biasPercent}% bias`
                      : `Plan ${plannedTarget} W`}
            </span>
            {overrideActive && (
              <button type="button" className="secondary override-reset" disabled={runner.status !== "running"} onClick={() => void backToPlan()}>
                Reset override{plannedTarget !== null ? ` · ${plannedTarget} W` : ""}
              </button>
            )}
            <div className="bias-stepper" aria-label="Workout bias">
              <span>Bias</span>
              <button type="button" className="secondary" onClick={() => void adjustBias(-BIAS_STEP_PERCENT)}>−</button>
              <strong className={biasPercent === 100 ? "" : "active"}>{biasPercent}%</strong>
              <button type="button" className="secondary" onClick={() => void adjustBias(BIAS_STEP_PERCENT)}>+</button>
              <button
                type="button"
                className={biasPercent === 100 ? "text-button bias-reset placeholder" : "text-button bias-reset"}
                disabled={biasPercent === 100}
                aria-hidden={biasPercent === 100}
                tabIndex={biasPercent === 100 ? -1 : 0}
                onClick={() => void perform(() => api.setBiasPercent(100), "reset workout bias")}
              >
                Reset
              </button>
            </div>
          </div>
        )}
      </div>
    ),
    powerChart: (
      <div className="card live-chart">
        <div className="chart-heading">
          <div><span className="label">FULL SESSION</span><h3>Power</h3></div>
          <button
            type="button"
            className="chart-collapse"
            aria-expanded={powerChartExpanded}
            aria-controls="ride-power-chart"
            onClick={() => setPowerChartExpanded((expanded) => !expanded)}
          >
            {powerChartExpanded ? "Collapse" : "Expand"}
            <ChevronDown size={17} />
          </button>
        </div>
        {powerChartExpanded && (
          <div id="ride-power-chart">
            <SessionAreaChart
              samples={chartHistory}
              dataKey="displayPowerWatts"
              unit="W"
              color="#c8ff32"
              name={powerSmoothing === "instant" ? "Power" : `Power (${powerSmoothingLabel[powerSmoothing]})`}
              domain={powerDomain}
            />
            {openEnded
              ? <div className="open-ended-time"><span>Elapsed</span><strong>{formatDuration(elapsed)}</strong><span>Open ended</span></div>
              : <div className="progress-meta"><span>{formatDuration(elapsed)}</span><div className="progress"><i style={{ width: `${progress}%` }} /></div><span>-{formatDuration(Math.max(0, (total ?? 0) - elapsed))}</span></div>}
          </div>
        )}
      </div>
    ),
    heartRateChart: (
      <div className="card live-chart secondary-chart">
        <div className="chart-heading">
          <div><span className="label">FULL SESSION</span><h3>Heart rate</h3></div>
          <button
            type="button"
            className="chart-collapse"
            aria-expanded={heartRateChartExpanded}
            aria-controls="ride-heart-rate-chart"
            onClick={() => setHeartRateChartExpanded((expanded) => !expanded)}
          >
            {heartRateChartExpanded ? "Collapse" : "Expand"}
            <ChevronDown size={17} />
          </button>
        </div>
        {heartRateChartExpanded && (
          <div id="ride-heart-rate-chart">
            <SessionAreaChart
              samples={chartHistory}
              dataKey="heartRateBpm"
              unit="bpm"
              color="#ff6f7d"
              name="Heart rate"
              domain={heartRateDomain}
            />
          </div>
        )}
      </div>
    ),
    deviceStats: (
      <DeviceStatsCard
        hub={hub}
        telemetry={telemetry}
        displayPowerWatts={displayedPower}
        powerSmoothing={powerSmoothing}
        historySampleCount={telemetryHistory.length}
        control={riding ? runner.control : undefined}
      />
    ),
    timeInZone: (
      <div className="zone-chart-grid">
        <TimeInZoneChart
          title="Power zones"
          zones={powerZones}
          seconds={powerZoneSeconds}
        />
        <TimeInZoneChart
          title="Heart-rate zones"
          zones={heartRateZones}
          seconds={heartRateZoneSeconds}
        />
      </div>
    ),
  } satisfies Record<RideCardId, ReactNode>;

  return (
    <>
      {!active && <PageHeader eyebrow="TRAINING ROOM" title="Start a ride" />}
      {riding && runner.recordingWarning && <RecordingWarning message={runner.recordingWarning} />}
      {runner.status === "finished" && runner.saveWarning && <RecordingWarning message={runner.saveWarning} />}
      {!connected && <div className="notice"><Bluetooth /><div><strong>No trainer connected</strong><p>Connect a trainer to begin.</p></div><button className="primary" onClick={onConnect}>Connect</button></div>}
      {riding && runner.control === "lost" && (
        <div className="notice" role="status">
          <Bluetooth />
          <div>
            <strong>Trainer link lost</strong>
            <p>{runner.status === "paused" ? "Reconnecting. The ride clock is paused; trainer pause will be confirmed when the connection returns." : "Reconnecting. The clock keeps running and the target is re-applied as soon as the trainer is back."}</p>
          </div>
        </div>
      )}
      {riding && runner.control === "degraded" && (
        <div className="notice" role="status">
          <Bluetooth />
          <div>
            <strong>{runner.status === "paused" ? "Trainer pause not confirmed" : "Trainer not acknowledging targets"}</strong>
            <p>{runner.status === "paused" ? "Retrying pause. The ride clock is paused, but the trainer may still be applying resistance." : "Retrying. The workout continues."}</p>
          </div>
        </div>
      )}
      {!active ? (
        <section className="ride-setup">
          <div className="card setup-card">
            <div className="free-ride-start">
              <div>
                <span className="label">MANUAL ERG</span>
                <h2>Free Ride</h2>
                <p>Start at 100 W, then use the arrow keys or on-screen controls to adjust by 5 W.</p>
              </div>
              <button className="primary" disabled={!connected} onClick={() => void perform(() => api.startFreeRide(), "start free ride")}><Play fill="currentColor" /> Start free ride</button>
            </div>
            <div className="setup-divider"><span>OR CHOOSE A WORKOUT</span></div>
            <span className="label">SELECT WORKOUT</span>
            <div className="select-list">
              {workouts.length === 0 ? (
                <div className="ride-empty-workouts">
                  <strong>No workouts saved yet</strong>
                  <span>Create one or import a ZWO file, then come back when you’re ready.</span>
                  <button className="secondary" onClick={onBrowseWorkouts}>Open workout library</button>
                </div>
              ) : workouts.map((workout) => <button key={workout.id} className={selectedWorkout === workout.id ? "selected" : ""} aria-pressed={selectedWorkout === workout.id} onClick={() => setSelectedWorkout(workout.id)}>
                <div><strong>{workout.name}</strong><span>{formatDuration(workoutDuration(workout.steps))}</span></div><WorkoutProfile steps={workout.steps} ftpWatts={profile.ftpWatts} powerZones={powerZones} /></button>)}
            </div>
            <button className="primary start-button" disabled={!connected || !selectedWorkout} onClick={() => selectedWorkout && void perform(() => api.startWorkout(selectedWorkout), "start workout")}><Play fill="currentColor" /> Start workout</button>
          </div>
          <div className="card ride-preview"><span className="label">WORKOUT PREVIEW</span><h2>{selected?.name ?? "Choose a workout"}</h2><p>{selected?.description}</p>{selected && <><div className="preview-chart"><WorkoutProfile variant="editor" steps={selected.steps} ftpWatts={profile.ftpWatts} powerZones={powerZones} /></div><div className="preview-stats"><Metric value={formatDuration(workoutDuration(selected.steps))} unit="duration" /><Metric value={`${selected.steps.length}`} unit="blocks" /></div></>}</div>
        </section>
      ) : (
        <section className="live-ride">
          <div className="live-ride-grid">
            {displayPreferences.cards.map((card) =>
              card.visible ? (
                <div
                  className={card.id === "power" || card.id === "cadence" || card.id === "speed" || card.id === "heartRate" ? "ride-card-cell compact" : "ride-card-cell wide"}
                  data-ride-card={card.id}
                  key={card.id}
                >
                  {cardContent[card.id]}
                </div>
              ) : null,
            )}
          </div>
        </section>
      )}
      {runner.status === "error" && runner.sessionId === null && (
        <div className="toast error-toast" role="alert">
          <span>
            Ride ended: {runner.message}.
          </span>
        </div>
      )}
    </>
  );
}

function HistoryPage({ sessions, selected, profile, trainingZones, onSelect, onClose, onExport, onExportFit, onGarmin }: { sessions: SessionSummary[]; selected: SessionDetail | null; profile: Profile; trainingZones: TrainingZoneSettings; onSelect: (session: SessionSummary) => void; onClose: () => void; onExport: (session: SessionSummary) => void; onExportFit: (session: SessionSummary) => void; onGarmin: (session: SessionSummary) => void }) {
  return (
    <>
      <PageHeader eyebrow={`${sessions.length} RECORDED RIDES`} title="Ride history" />
      {sessions.length === 0 ? <div className="empty-state"><History /><h2>No rides yet</h2><p>Completed and stopped workouts appear here automatically.</p></div> :
      <div className="history-list">{sessions.map((session) => <button key={session.id} className="history-row" onClick={() => onSelect(session)}>
        <span className={session.completed ? "completion complete" : "completion"}>{session.completed ? "✓" : "–"}</span>
        <div className="history-title"><strong>{session.workoutName}</strong>{session.recordingWarning && <span>Recording warning</span>}<span>{new Date(session.startedAt).toLocaleString()}</span></div>
        <Metric value={formatDuration(session.elapsedSeconds)} unit="duration" /><Metric value={`${session.averagePowerWatts}`} unit="W avg" /><DistanceMetric meters={session.estimatedDistanceMeters} unit={profile.distanceUnit} /><ChevronRight />
      </button>)}</div>}
      {selected && (
        <RideDetailModal
          session={selected}
          profile={profile}
          trainingZones={trainingZones}
          onClose={onClose}
          onExport={onExport}
          onExportFit={onExportFit}
          onGarmin={onGarmin}
        />
      )}
    </>
  );
}

function RecordingWarning({ message }: { message: string }) {
  return <div className="notice" role="alert"><div><strong>Ride data needs attention</strong><p>{message}</p></div></div>;
}

export function PostRidePrompt({ errorMessage, saveWarning = null, onView, onDismiss }: { errorMessage: string | null; saveWarning?: string | null; onView: () => void; onDismiss: () => void }) {
  const warning = saveWarning ?? errorMessage;
  return (
    <div className={warning ? "toast post-ride-prompt error-toast" : "toast post-ride-prompt success-toast"} role={warning ? "alert" : "status"}>
      <div className="post-ride-copy">
        <strong>{saveWarning ? "Ride ended with a save problem" : errorMessage ? "Ride ended" : "Ride saved. Nice work!"}</strong>
        <span>{warning ?? "Your effort is safely recorded and ready to review."}</span>
      </div>
      <div className="post-ride-actions">
        <button className="primary" onClick={onView}>View ride metrics</button>
        <button className="icon-button" onClick={onDismiss} aria-label="Dismiss ride summary"><X size={17} /></button>
      </div>
    </div>
  );
}

export function RideDetailModal({ session, profile, trainingZones, onClose, onExport, onExportFit, onGarmin }: { session: SessionDetail; profile: Profile; trainingZones: TrainingZoneSettings; onClose: () => void; onExport: (session: SessionSummary) => void; onExportFit: (session: SessionSummary) => void; onGarmin: (session: SessionSummary) => void }) {
  const [range, setRange] = useState<ChartRange | null>(null);
  const clearRange = useCallback(() => setRange(null), []);
  // Escape dismisses the innermost thing: a chart selection first, then the modal.
  const dialogRef = useDialog<HTMLDivElement>(range ? clearRange : onClose);
  const powerZones = useMemo(
    () => effectivePowerZones(trainingZones, profile.ftpWatts),
    [profile.ftpWatts, trainingZones],
  );
  const heartRateZones = useMemo(
    () => effectiveHeartRateZones(trainingZones, profile.maxHeartRateBpm),
    [profile.maxHeartRateBpm, trainingZones],
  );
  const activeSamples = useMemo(
    () => withActiveElapsed(session.samples, session.summary.elapsedSeconds),
    [session],
  );
  const samples = useMemo(() => downsampleTelemetry(activeSamples), [activeSamples]);
  const rangeSummary = useMemo(
    () => (range ? summarizeRange(activeSamples, range) : null),
    [activeSamples, range],
  );
  const powerSeconds = useMemo(
    () => timeInZones(session.samples, powerZones, "power"),
    [powerZones, session],
  );
  const heartRateSeconds = useMemo(
    () => timeInZones(session.samples, heartRateZones, "heartRate"),
    [heartRateZones, session],
  );
  return (
    <div className="modal-backdrop dialog-enter" role="presentation" onMouseDown={(event) => {
      if (event.target === event.currentTarget) onClose();
    }}>
      <div ref={dialogRef} className="modal detail-modal" role="dialog" aria-modal="true" aria-labelledby="ride-detail-title" tabIndex={-1}>
        <button className="modal-close" onClick={onClose} aria-label="Close ride detail"><X /></button>
        <div className="detail-header">
          <div>
            <span className="label">RIDE DETAIL</span>
            <h2 id="ride-detail-title">{session.summary.workoutName}</h2>
            <p>{new Date(session.summary.startedAt).toLocaleString()}</p>
          </div>
          <div className="detail-action-block">
            <div className="detail-actions">
              <button className="primary" onClick={() => onGarmin(session.summary)}><Upload size={16}/> Upload to Garmin</button>
              <button className="secondary" onClick={() => onExportFit(session.summary)}><Download size={16}/> Export FIT</button>
              <button className="secondary" onClick={() => onExport(session.summary)}><Download size={16}/> Export CSV</button>
            </div>
            <p className="handoff-note">Garmin Connect and Finder will open. Drag the selected FIT file onto Garmin’s import page, then confirm the upload.</p>
          </div>
        </div>
        {session.summary.recordingWarning && <RecordingWarning message={session.summary.recordingWarning} />}
        <div className="detail-metrics">
          <Metric value={formatDuration(session.summary.elapsedSeconds)} unit="duration" />
          <Metric value={`${session.summary.averagePowerWatts}`} unit="W average" />
          <Metric value={`${session.summary.maxPowerWatts}`} unit="W maximum" />
          <Metric value={`${Math.round(session.summary.averageCadenceRpm ?? 0)}`} unit="rpm average" />
          <DistanceMetric meters={session.summary.estimatedDistanceMeters} unit={profile.distanceUnit} />
        </div>
        <p className="distance-note">Estimated distance · {distanceSourceLabel(session.summary.distanceSource)}</p>
        <div className="history-charts">
          <RangeSummaryStrip summary={rangeSummary} onClear={clearRange} />
          <h3>Power</h3>
          <SessionAreaChart samples={samples} dataKey="powerWatts" unit="W" color="#c8ff32" name="Power" domain={powerDomain} selection={range} onSelectionChange={setRange}/>
          <h3>Heart rate</h3>
          <SessionAreaChart samples={samples} dataKey="heartRateBpm" unit="bpm" color="#ff6f7d" name="Heart rate" domain={heartRateDomain} selection={range} onSelectionChange={setRange}/>
          <div className="zone-chart-grid history-zone-charts">
            <TimeInZoneChart title="Power zones" zones={powerZones} seconds={powerSeconds}/>
            <TimeInZoneChart title="Heart-rate zones" zones={heartRateZones} seconds={heartRateSeconds}/>
          </div>
        </div>
      </div>
    </div>
  );
}

const KG_PER_LB = 0.45359237;

const rideCardLabels: Record<RideCardId, string> = {
  power: "Power",
  cadence: "Cadence",
  speed: "Speed",
  heartRate: "Heart rate",
  workoutTimeline: "Workout timeline",
  targetAndBias: "Target & bias",
  powerChart: "Power chart",
  heartRateChart: "Heart-rate chart",
  timeInZone: "Time in zone",
  deviceStats: "Stats for nerds",
};

function displayedWeight(kg: number, unit: Profile["weightUnit"]) {
  return Number((unit === "lb" ? kg / KG_PER_LB : kg).toFixed(1));
}

function storedWeight(value: number, unit: Profile["weightUnit"]) {
  return unit === "lb" ? value * KG_PER_LB : value;
}

export function SettingsPage({
  profile,
  trainingZones,
  rideDisplayPreferences,
  devMode,
  perform,
  onProfileUpdate,
  onTrainingZonesUpdate,
  onSave,
  onSaveTrainingZones,
  onSaveRideDisplayPreferences,
  onForgetDevices,
  onDevMode,
}: {
  profile: Profile;
  trainingZones: TrainingZoneSettings;
  rideDisplayPreferences: RideDisplayPreferences;
  devMode: boolean;
  perform: (action: () => Promise<unknown>, label?: string) => Promise<void>;
  onProfileUpdate: (profile: Profile) => void;
  onTrainingZonesUpdate: (zones: TrainingZoneSettings) => void;
  onSave: (profile: Profile) => void;
  onSaveTrainingZones: (zones: TrainingZoneSettings) => void;
  onSaveRideDisplayPreferences: (preferences: RideDisplayPreferences) => void;
  onForgetDevices: () => Promise<void>;
  onDevMode: (enabled: boolean) => void;
}) {
  const [draft, setDraft] = useState(profile);
  const [zoneDraft, setZoneDraft] = useState(trainingZones);
  const [rideDisplayDraft, setRideDisplayDraft] = useState(
    rideDisplayPreferences,
  );
  const [logPath, setLogPath] = useState("Loading log location…");
  const [rideFilesPath, setRideFilesPath] = useState("Loading ride files location…");
  const [apiKey, setApiKey] = useState("");
  const [intervalsConfigured, setIntervalsConfigured] = useState(false);
  const [intervalsBusy, setIntervalsBusy] = useState<"save" | "clear" | "refresh" | null>(null);
  const [intervalsRefreshStatus, setIntervalsRefreshStatus] = useState<string | null>(null);
  useEffect(() => {
    void api.logFilePath().then(setLogPath);
    void api.rideFilesPath().then(setRideFilesPath);
    void perform(async () => {
      setIntervalsConfigured(await api.intervalsApiKeyConfigured());
    }, "load Intervals.icu settings");
  }, [perform]);
  useEffect(() => setDraft(profile), [profile]);
  useEffect(() => setZoneDraft(trainingZones), [trainingZones]);
  useEffect(
    () => setRideDisplayDraft(rideDisplayPreferences),
    [rideDisplayPreferences],
  );

  const moveRideCard = (index: number, direction: -1 | 1) => {
    const nextIndex = index + direction;
    if (nextIndex < 0 || nextIndex >= rideDisplayDraft.cards.length) return;
    const cards = [...rideDisplayDraft.cards];
    [cards[index], cards[nextIndex]] = [cards[nextIndex], cards[index]];
    setRideDisplayDraft({ version: 2, cards });
  };

  const saveIntervalsKey = async () => {
    setIntervalsBusy("save");
    await perform(async () => {
      await api.saveIntervalsApiKey(apiKey);
      setIntervalsConfigured(true);
      setApiKey("");
    }, "save Intervals.icu API key");
    setIntervalsBusy(null);
  };

  const clearIntervalsKey = async () => {
    setIntervalsBusy("clear");
    await perform(async () => {
      await api.clearIntervalsApiKey();
      setIntervalsConfigured(false);
      setApiKey("");
    }, "clear Intervals.icu API key");
    setIntervalsBusy(null);
  };

  const refreshEstimatedFtp = async () => {
    setIntervalsBusy("refresh");
    await perform(async () => {
      await api.saveTrainingZones(zoneDraft);
      const result = await api.refreshEstimatedFtp();
      onProfileUpdate(result.profile);
      onTrainingZonesUpdate(result.zones);
      setIntervalsRefreshStatus(
        result.powerZonesImported
          ? "FTP, heart-rate zones, and power zones updated."
          : result.heartRateZonesImported
            ? "FTP and heart-rate zones updated. Power zones were not imported."
            : "FTP updated. Intervals.icu did not return usable training zones.",
      );
    }, "refresh training settings");
    setIntervalsBusy(null);
  };

  return (
    <>
      <PageHeader eyebrow="LOCAL PROFILE" title="Settings" />
      <section className="card settings-card"><div><span className="label">RIDER PROFILE</span><h2>Training and distance</h2><p>Your weight and bike weight support flat-road distance estimates when the trainer does not report speed. Values are stored in kilograms regardless of display units.</p></div><form onSubmit={(event) => { event.preventDefault(); onSave(draft); }}>
        <label>Rider name<input value={draft.name} onChange={(event) => setDraft({ ...draft, name: event.target.value })}/></label>
        <div className="form-row"><label>FTP (watts)<EditableNumberInput min={50} max={500} value={draft.ftpWatts} onValueChange={(ftpWatts) => setDraft({ ...draft, ftpWatts })}/></label><label>Maximum heart rate (bpm)<EditableNumberInput min={100} max={230} value={draft.maxHeartRateBpm} onValueChange={(maxHeartRateBpm) => setDraft({ ...draft, maxHeartRateBpm })}/></label></div>
        <label>Safety power limit<EditableNumberInput min={100} max={2500} value={draft.maxPowerWatts} onValueChange={(maxPowerWatts) => setDraft({ ...draft, maxPowerWatts })}/></label>
        <div className="form-row"><label>Weight unit<select value={draft.weightUnit} onChange={(event) => setDraft({ ...draft, weightUnit: event.target.value as Profile["weightUnit"] })}><option value="kg">Kilograms (kg)</option><option value="lb">Pounds (lb)</option></select></label><label>Distance unit<select value={draft.distanceUnit} onChange={(event) => setDraft({ ...draft, distanceUnit: event.target.value as Profile["distanceUnit"] })}><option value="km">Kilometers</option><option value="mi">Miles</option></select></label></div>
        <div className="form-row"><label>Rider weight ({draft.weightUnit})<EditableNumberInput step={0.1} min={draft.weightUnit === "lb" ? 66 : 30} max={draft.weightUnit === "lb" ? 551 : 250} value={displayedWeight(draft.riderWeightKg, draft.weightUnit)} onValueChange={(value) => setDraft({ ...draft, riderWeightKg: storedWeight(value, draft.weightUnit) })}/></label><label>Bike weight ({draft.weightUnit})<EditableNumberInput step={0.1} min={draft.weightUnit === "lb" ? 7 : 3} max={draft.weightUnit === "lb" ? 88 : 40} value={displayedWeight(draft.bikeWeightKg, draft.weightUnit)} onValueChange={(value) => setDraft({ ...draft, bikeWeightKg: storedWeight(value, draft.weightUnit) })}/></label></div>
        <button className="primary" type="submit">Save settings</button>
      </form></section>
      <section className="card settings-card">
        <div>
          <span className="label">RIDE LAYOUT</span>
          <h2>Live ride cards</h2>
          <p>Choose which cards appear during a ride and arrange them in the order you want. Stats for nerds is off by default; turn it on to see what every connected device is reporting while you ride.</p>
        </div>
        <div className="ride-layout-settings">
          <div className="ride-card-list">
            {rideDisplayDraft.cards.map((card, index) => (
              <div className="ride-card-setting" key={card.id}>
                <label>
                  <input
                    type="checkbox"
                    checked={card.visible}
                    onChange={(event) => {
                      const cards = rideDisplayDraft.cards.map((current) =>
                        current.id === card.id
                          ? { ...current, visible: event.target.checked }
                          : current,
                      );
                      setRideDisplayDraft({ version: 2, cards });
                    }}
                  />
                  <span>{rideCardLabels[card.id]}</span>
                </label>
                <div className="ride-card-order">
                  <button
                    type="button"
                    className="icon-button"
                    aria-label={`Move ${rideCardLabels[card.id]} up`}
                    disabled={index === 0}
                    onClick={() => moveRideCard(index, -1)}
                  >
                    <ChevronUp size={17} />
                  </button>
                  <button
                    type="button"
                    className="icon-button"
                    aria-label={`Move ${rideCardLabels[card.id]} down`}
                    disabled={index === rideDisplayDraft.cards.length - 1}
                    onClick={() => moveRideCard(index, 1)}
                  >
                    <ChevronDown size={17} />
                  </button>
                </div>
              </div>
            ))}
          </div>
          <div className="settings-actions">
            <button
              type="button"
              className="secondary"
              onClick={() =>
                setRideDisplayDraft({
                  version: 2,
                  cards: defaultRideDisplayPreferences.cards.map((card) => ({
                    ...card,
                  })),
                })
              }
            >
              Reset to default
            </button>
            <button
              type="button"
              className="primary"
              onClick={() => onSaveRideDisplayPreferences(rideDisplayDraft)}
            >
              Save ride layout
            </button>
          </div>
        </div>
      </section>
      <section className="card settings-card">
        <div>
          <span className="label">TRAINING ZONES</span>
          <h2>Power and heart rate</h2>
          <p>Defaults follow your FTP and maximum heart rate. Editing a boundary switches that set to custom values.</p>
        </div>
        <div className="zone-settings">
          <label className="settings-toggle">
            <input
              type="checkbox"
              checked={zoneDraft.syncPowerZonesFromIntervals}
              onChange={(event) =>
                setZoneDraft({
                  ...zoneDraft,
                  syncPowerZonesFromIntervals: event.target.checked,
                })
              }
            />
            <span>
              Import power zones from Intervals.icu
              <small>Applied when training settings are refreshed below.</small>
            </span>
          </label>
          <ZoneEditor
            title="Power"
            unit="W"
            mode={zoneDraft.powerMode}
            zones={effectivePowerZones(zoneDraft, draft.ftpWatts)}
            onChange={(zones) => setZoneDraft({ ...zoneDraft, powerMode: "custom", powerZones: zones })}
            onReset={() => setZoneDraft({ ...zoneDraft, powerMode: "derived", powerZones: [] })}
          />
          <ZoneEditor
            title="Heart rate"
            unit="bpm"
            mode={zoneDraft.heartRateMode}
            zones={effectiveHeartRateZones(zoneDraft, draft.maxHeartRateBpm)}
            onChange={(zones) => setZoneDraft({ ...zoneDraft, heartRateMode: "custom", heartRateZones: zones })}
            onReset={() => setZoneDraft({ ...zoneDraft, heartRateMode: "derived", heartRateZones: [] })}
          />
          <button className="primary" type="button" onClick={() => onSaveTrainingZones(zoneDraft)}>Save zone settings</button>
        </div>
      </section>
      <section className="card settings-card">
        <div>
          <span className="label">INTERVALS.ICU</span>
          <h2>FTP and training zones</h2>
          <p>Connect your Intervals.icu account to import modeled eFTP, cycling heart-rate zones, and power zones when enabled above.</p>
          <span className={`integration-status ${intervalsConfigured ? "configured" : ""}`}>
            {intervalsConfigured ? "API key saved" : "API key not configured"}
          </span>
        </div>
        <div className="integration-controls">
          <label>
            API key
            <input
              type="password"
              autoComplete="off"
              placeholder={intervalsConfigured ? "Enter a new key to replace the saved key" : "Intervals.icu API key"}
              value={apiKey}
              onChange={(event) => setApiKey(event.target.value)}
            />
          </label>
          <p className="settings-note">The key is stored in this app's local SQLite database and is used only by the desktop backend.</p>
          <div className="settings-actions">
            <button
              className="secondary"
              disabled={!apiKey.trim() || intervalsBusy !== null}
              onClick={() => void saveIntervalsKey()}
            >
              {intervalsBusy === "save" ? "Saving…" : "Save API key"}
            </button>
            {intervalsConfigured && (
              <button
                className="danger-button"
                disabled={intervalsBusy !== null}
                onClick={() => void clearIntervalsKey()}
              >
                {intervalsBusy === "clear" ? "Clearing…" : "Clear key"}
              </button>
            )}
          </div>
          <button
            className="primary"
            disabled={!intervalsConfigured || intervalsBusy !== null}
            onClick={() => void refreshEstimatedFtp()}
          >
            {intervalsBusy === "refresh" ? "Refreshing…" : "Refresh training settings"}
          </button>
          {intervalsRefreshStatus && <p className="integration-result">{intervalsRefreshStatus}</p>}
        </div>
      </section>
      <section className="card settings-card"><div><span className="label">DATA & DIAGNOSTICS</span><h2>Local-first by design</h2><p>Every finalized ride is stored in SQLite and as a persistent Garmin-compatible FIT file. Missing FIT files are regenerated automatically.</p></div><div className="data-locations"><div className="log-location"><span>Ride Files</span><code>{rideFilesPath}</code><button className="secondary" onClick={() => void api.revealRideFiles().catch(() => undefined)}>Show Ride Files</button></div><div className="log-location"><span>Log file</span><code>{logPath}</code><button className="secondary" onClick={() => void api.revealLogFile().catch(() => undefined)}>Show in folder</button><button className="secondary" onClick={() => void navigator.clipboard.writeText(logPath)}>Copy path</button></div><div className="log-location"><span>Known devices</span><p className="settings-note">Devices you have connected are remembered on this computer so they can be reconnected without scanning. Forgetting them does not disconnect anything.</p><button className="danger-button" onClick={() => void onForgetDevices()}>Forget all devices</button></div></div></section>
      <section className="card settings-card">
        <div>
          <span className="label">DEVELOPER</span>
          <h2>Developer mode</h2>
          <p>Off by default. Turn it on to work on blake.bike without a trainer in the room; leave it off for real rides so a scan only ever offers your own hardware.</p>
        </div>
        <label className="settings-toggle">
          <input
            type="checkbox"
            checked={devMode}
            onChange={(event) => onDevMode(event.target.checked)}
          />
          <span>
            Offer simulated devices
            <small>Adds the simulated trainer, HR strap, power meter and cadence sensor to device scans. A simulator that is already connected stays connected until you disconnect it.</small>
          </span>
        </label>
      </section>
      <AboutCard />
    </>
  );
}

function AboutCard() {
  const [version, setVersion] = useState<string | null>(null);
  const [showNotes, setShowNotes] = useState(false);
  useEffect(() => {
    void api
      .appVersion()
      .then(setVersion)
      .catch(() => undefined);
  }, []);
  const release = releaseFor(version);
  const released = formatReleaseDate(release?.date ?? null);
  return (
    <section className="card settings-card about-card">
      <div>
        <span className="label">ABOUT</span>
        <h2>blake.bike {version ?? release?.version ?? ""}</h2>
        {release?.summary && <p>{release.summary}</p>}
        {released && <p className="settings-note">Released {released}</p>}
      </div>
      <div className="settings-actions">
        <button
          className="secondary"
          disabled={releases.length === 0}
          onClick={() => setShowNotes(true)}
        >
          <Info size={16} /> Release notes
        </button>
        <button className="secondary" onClick={() => void api.openWebsite().catch(() => undefined)}>
          <ExternalLink size={16} /> blake.bike
        </button>
      </div>
      {showNotes && (
        <ReleaseNotesModal
          currentVersion={version ?? release?.version ?? null}
          onClose={() => setShowNotes(false)}
        />
      )}
    </section>
  );
}

export function ReleaseNotesModal({
  currentVersion,
  onClose,
}: {
  currentVersion: string | null;
  onClose: () => void;
}) {
  return (
    <div className="modal-backdrop">
      <div className="modal release-notes-modal">
        <button className="modal-close" onClick={onClose} aria-label="Close release notes"><X /></button>
        <span className="label">RELEASE NOTES</span>
        <h2>What's new in blake.bike</h2>
        <div className="release-list">
          {releases.map((release) => (
            <ReleaseEntry
              key={release.version}
              release={release}
              installed={release.version === currentVersion}
            />
          ))}
        </div>
        <div className="modal-actions">
          <button className="primary" onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  );
}

function ReleaseEntry({ release, installed }: { release: Release; installed: boolean }) {
  const released = formatReleaseDate(release.date);
  return (
    <article className="release-entry">
      <header>
        <h3>
          {release.version}
          {installed && <span className="release-installed">Installed</span>}
        </h3>
        {released && <span className="release-date">{released}</span>}
      </header>
      {release.summary && <p className="release-summary">{release.summary}</p>}
      {release.sections.map((section) => (
        <div className="release-section" key={section.title}>
          <span className="label">{section.title.toUpperCase()}</span>
          <ul>
            {section.items.map((item) => (
              <li key={item}>{item}</li>
            ))}
          </ul>
        </div>
      ))}
    </article>
  );
}

type ChartDomain = [number | string, number | string];
const powerDomain: ChartDomain = [0, "dataMax + 50"];
const heartRateDomain: ChartDomain = ["dataMin - 10", "dataMax + 10"];

// Recharts' default tooltip is a white box; on the dark theme the lime and
// pink series values were unreadable on it. Match the modal surface instead
// and let each series keep its own color for the value.
const chartTooltipContentStyle = {
  background: "#1b1f1a",
  border: "1px solid #373d34",
  borderRadius: 9,
  boxShadow: "0 14px 40px rgba(0,0,0,.5)",
  padding: "8px 12px",
} as const;
const chartTooltipLabelStyle = { color: "#c9d0c3", fontSize: 12, marginBottom: 4 } as const;
const chartTooltipItemStyle = { fontSize: 13, padding: 0 } as const;
const chartTooltipCursor = { stroke: "#6b7566", strokeWidth: 1 } as const;

const formatChartTime = (value: unknown) =>
  formatDuration(Math.max(0, Math.round(Number(value) / 1000)));

const noSelectionChange = () => undefined;

const SessionAreaChart = memo(function SessionAreaChart({
  samples,
  dataKey,
  unit,
  color,
  name,
  domain,
  selection = null,
  onSelectionChange,
}: {
  samples: Array<Telemetry & { displayPowerWatts?: number; activeElapsedMs: number }>;
  dataKey: "powerWatts" | "displayPowerWatts" | "heartRateBpm";
  unit: string;
  color: string;
  name: string;
  domain: ChartDomain;
  /** Highlighted stretch of the ride, shared by every chart on the screen. */
  selection?: ChartRange | null;
  /** When provided, the rider can press and drag on the chart to select a range. */
  onSelectionChange?: (range: ChartRange | null) => void;
}) {
  const gradientId = `fill-${dataKey}`;
  const selectable = onSelectionChange !== undefined;
  const drag = useChartRangeDrag(onSelectionChange ?? noSelectionChange);
  return (
    <div className={selectable ? "session-chart session-chart-selectable" : "session-chart"}>
      <ResponsiveContainer width="100%" height={220}>
        <AreaChart data={samples} {...(selectable ? drag : undefined)}>
          <defs>
            <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor={color} stopOpacity={0.42} />
              <stop offset="100%" stopColor={color} stopOpacity={0} />
            </linearGradient>
          </defs>
          <CartesianGrid strokeDasharray="4 4" vertical={false} />
          <XAxis
            dataKey="activeElapsedMs"
            type="number"
            domain={["dataMin", "dataMax"]}
            tickFormatter={formatChartTime}
            minTickGap={45}
          />
          <YAxis width={42} domain={domain} />
          <Tooltip
            labelFormatter={formatChartTime}
            formatter={(value) => [`${value ?? "—"} ${unit}`, name]}
            contentStyle={chartTooltipContentStyle}
            labelStyle={chartTooltipLabelStyle}
            itemStyle={chartTooltipItemStyle}
            cursor={chartTooltipCursor}
          />
          {selection && (
            <ReferenceArea
              x1={selection.startMs}
              x2={selection.endMs}
              ifOverflow="discard"
              fill={color}
              fillOpacity={0.16}
              stroke={color}
              strokeOpacity={0.6}
            />
          )}
          <Area
            connectNulls={false}
            type="monotone"
            dataKey={dataKey}
            stroke={color}
            fill={`url(#${gradientId})`}
            isAnimationActive={false}
          />
        </AreaChart>
      </ResponsiveContainer>
    </div>
  );
});

/**
 * Average, maximum, and minimum for the stretch highlighted on the charts.
 * Shows a short hint until the rider drags a range.
 */
export function RangeSummaryStrip({
  summary,
  onClear,
}: {
  summary: RangeSummary | null;
  onClear: () => void;
}) {
  if (!summary) {
    return <p className="range-hint">Drag across a chart to see the average, maximum, and minimum for that section.</p>;
  }
  const { range, durationMs, power, heartRate } = summary;
  return (
    <div className="range-summary" role="status">
      <div className="range-summary-span">
        <span className="label">SELECTION</span>
        <strong>{formatChartTime(range.startMs)} – {formatChartTime(range.endMs)}</strong>
        <span>{formatChartTime(durationMs)} selected</span>
      </div>
      <Metric value={power ? `${power.average}` : "—"} unit="W average" />
      <Metric value={power ? `${power.max}` : "—"} unit="W maximum" />
      <Metric value={power ? `${power.min}` : "—"} unit="W minimum" />
      <Metric value={heartRate ? `${heartRate.average}` : "—"} unit="bpm average" />
      <Metric value={heartRate ? `${heartRate.max}` : "—"} unit="bpm maximum" />
      <Metric value={heartRate ? `${heartRate.min}` : "—"} unit="bpm minimum" />
      <button type="button" className="secondary" onClick={onClear}>Clear</button>
    </div>
  );
}

const TimeInZoneChart = memo(function TimeInZoneChart({
  title,
  zones,
  seconds,
}: {
  title: string;
  zones: ReturnType<typeof effectivePowerZones>;
  seconds: number[];
}) {
  const data = zones.map((zone, index) => ({
    name: zone.name,
    seconds: Math.round(seconds[index] ?? 0),
    fill: zoneColorAt(index),
  }));
  return (
    <div className="card zone-chart">
      <div className="chart-heading"><div><span className="label">TIME IN ZONE</span><h3>{title}</h3></div></div>
      <ResponsiveContainer width="100%" height={Math.max(150, data.length * 30)}>
        <BarChart data={data} layout="vertical" margin={{ left: 4, right: 12 }}>
          <XAxis type="number" hide />
          <YAxis type="category" dataKey="name" width={58} tick={{ fontSize: 10 }} />
          <Tooltip
            formatter={(value) => [formatDuration(Number(value)), "Time"]}
            contentStyle={chartTooltipContentStyle}
            labelStyle={chartTooltipLabelStyle}
            itemStyle={chartTooltipItemStyle}
            cursor={{ fill: "rgba(255,255,255,.06)" }}
          />
          <Bar dataKey="seconds" radius={[0, 4, 4, 0]} isAnimationActive={false}>
            {data.map((entry) => <Cell key={entry.name} fill={entry.fill} />)}
          </Bar>
        </BarChart>
      </ResponsiveContainer>
    </div>
  );
});

function ZoneEditor({
  title,
  unit,
  mode,
  zones,
  onChange,
  onReset,
}: {
  title: string;
  unit: string;
  mode: "derived" | "custom";
  zones: ReturnType<typeof effectivePowerZones>;
  onChange: (zones: ReturnType<typeof effectivePowerZones>) => void;
  onReset: () => void;
}) {
  const update = (index: number, patch: Partial<(typeof zones)[number]>) =>
    onChange(zones.map((zone, zoneIndex) => zoneIndex === index ? { ...zone, ...patch } : zone));
  return (
    <div className="zone-editor">
      <div className="zone-editor-head">
        <strong>{title}</strong>
        <span>{mode === "derived" ? "Derived" : "Custom"}</span>
        {mode === "custom" && <button type="button" className="text-button" onClick={onReset}>Reset defaults</button>}
      </div>
      <div className="zone-boundaries">
        {zones.map((zone, index) => (
          <div className="zone-boundary" key={`${title}-${index}`}>
            <i style={{ background: zoneColorAt(index) }} />
            <input
              aria-label={`${title} zone ${index + 1} name`}
              value={zone.name}
              onChange={(event) => update(index, { name: event.target.value })}
            />
            {zone.upperBound === null ? (
              <span>and above</span>
            ) : (
              <label>
                up to
                <EditableNumberInput
                  aria-label={`${title} zone ${index + 1} upper bound`}
                  min={unit === "W" ? 1 : 30}
                  max={unit === "W" ? 3000 : 250}
                  value={zone.upperBound}
                  onValueChange={(upperBound) => update(index, { upperBound })}
                />
                {unit}
              </label>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

function WorkoutTimeline({
  steps,
  ftpWatts,
  biasPercent,
  powerZones,
  intervals,
  workoutName,
  currentIndex,
  intervalElapsedSeconds,
  elapsedSeconds,
  totalSeconds,
  paused,
  onSkip,
}: {
  steps: WorkoutStep[];
  ftpWatts: number;
  biasPercent: number;
  powerZones: readonly ZoneDefinition[];
  /** Runner-aligned, bias-applied intervals; index-for-index the same blocks the profile draws. */
  intervals: WorkoutInterval[];
  workoutName: string;
  currentIndex: number;
  intervalElapsedSeconds: number;
  elapsedSeconds: number;
  totalSeconds: number;
  paused: boolean;
  onSkip: () => void;
}) {
  const current = intervals[currentIndex];
  if (!current) return null;

  const blockState = (_shape: unknown, index: number) => {
    const state = index < currentIndex ? "completed" : index === currentIndex ? "current" : "upcoming";
    const progress = state === "completed"
      ? 1
      : state === "current"
        ? Math.min(1, intervalElapsedSeconds / Math.max(1, current.durationSeconds))
        : 0;
    return { className: state, state, progress };
  };
  const currentRemaining = Math.max(0, current.durationSeconds - intervalElapsedSeconds);
  const workoutRemaining = Math.max(0, totalSeconds - elapsedSeconds);
  const currentTarget = formatIntervalTarget(current.startWatts, current.endWatts);

  return (
    <section className="card workout-timeline" aria-label="Workout timeline">
      <div className="timeline-heading">
        <div>
          <span className="label">{workoutName}</span>
          <h3>Block {currentIndex + 1} of {intervals.length}</h3>
          <span className="timeline-target">{currentTarget}{paused ? " · Paused" : ""}</span>
        </div>
        <div className="timeline-side">
          <button className="secondary" onClick={onSkip}><SkipForward /> Skip block</button>
          <div className="timeline-countdowns">
            <div>
              <span>Current block</span>
              <strong>{formatDuration(currentRemaining)}</strong>
            </div>
            <div>
              <span>Workout remaining</span>
              <strong>{formatDuration(workoutRemaining)}</strong>
            </div>
          </div>
        </div>
      </div>
      <div className="workout-timeline-scroll">
        <div
          className="workout-timeline-track"
          style={{ width: `${Math.max(100, intervals.length * 3)}%` }}
        >
          <WorkoutProfile
            steps={steps}
            ftpWatts={ftpWatts}
            biasPercent={biasPercent}
            powerZones={powerZones}
            labels
            segmentState={blockState}
          />
        </div>
      </div>
    </section>
  );
}

function DistanceMetric({ meters, unit }: { meters: number; unit: Profile["distanceUnit"] }) {
  const formatted = formatDistance(meters, unit);
  return <Metric value={formatted.value} unit={`${formatted.unit} estimated`} />;
}

function distanceSourceLabel(source: SessionSummary["distanceSource"]) {
  if (source === "trainer") return "trainer speed";
  if (source === "mixed") return "trainer speed and power model";
  if (source === "power") return "flat-road power model";
  return "no telemetry";
}

function LiveMetric({ icon: Icon, label, value, unit, accent = false, note, control }: { icon: typeof Activity; label: string; value: string | number; unit: string; accent?: boolean; note?: string | null; control?: React.ReactNode }) {
  return <div className={accent ? "live-metric accent" : "live-metric"}><span><Icon size={17}/>{label}{control && <span className="metric-control">{control}</span>}</span><strong>{value}<small>{unit}</small></strong>{note && <em className="metric-source">{note}</em>}</div>;
}

function newWorkout(): Workout {
  const now = new Date().toISOString();
  return { id: crypto.randomUUID(), name: "New workout", description: "", source: "local", version: 1, createdAt: now, updatedAt: now, steps: [{ kind: "steady", durationSeconds: 300, target: { unit: "percentFtp", value: 70 } }] };
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function workingCopy(label: string) {
  if (label.includes("connect")) return "Connecting…";
  if (label.includes("scan")) return "Looking for devices…";
  if (label.includes("export")) return "Preparing export…";
  if (label.includes("refresh")) return "Refreshing training settings…";
  if (label.includes("stop workout")) return "Saving your ride…";
  if (label.includes("save")) return "Saving…";
  if (label.includes("delete") || label.includes("forget")) return "Removing…";
  if (label.includes("restore")) return "Restoring…";
  return "Working…";
}

function successCopy(label: string) {
  if (label.includes("connect")) return label.includes("disconnect") ? "Disconnected" : "Connected and ready";
  if (label.includes("scan")) return "Scan complete";
  if (label.includes("export")) return "Export ready";
  if (label.includes("refresh")) return "Training settings refreshed";
  if (label.includes("stop workout")) return "Ride saved. Nice work!";
  if (label.includes("save")) return "Saved";
  if (label.includes("delete") || label.includes("forget")) return "Removed";
  if (label.includes("restore")) return "Restored";
  return "Done";
}
