import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Activity,
  Bike,
  Bluetooth,
  ChevronRight,
  CircleStop,
  Download,
  Gauge,
  HeartPulse,
  History,
  Library,
  Pause,
  Play,
  Plus,
  Radio,
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
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { api } from "./api";
import type {
  DeviceRole,
  DeviceState,
  DevicesSnapshot,
  Profile,
  RunnerState,
  SessionDetail,
  SessionSummary,
  Telemetry,
  Workout,
  WorkoutStep,
} from "./types";
import {
  deviceRoleLabel,
  deviceRoles,
  formatDistance,
  formatDuration,
  formatSpeed,
  manualPowerDeltaForKey,
  workoutDuration,
} from "./types";
import { DevicePicker } from "./DevicePicker";
import { DevicesPage } from "./DevicesPage";
import { isConnected as slotConnected, sourceNote } from "./devices";
import "./App.css";

type Page = "home" | "workouts" | "devices" | "ride" | "history" | "settings";

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
  const [error, setError] = useState<string | null>(null);
  const [devicePicker, setDevicePicker] = useState<DeviceRole | null>(null);
  const [editor, setEditor] = useState<Workout | null>(null);
  const [selectedWorkout, setSelectedWorkout] = useState<string | null>(null);
  const [selectedSession, setSelectedSession] = useState<SessionDetail | null>(
    null,
  );
  const importRef = useRef<HTMLInputElement>(null);

  const load = useCallback(async () => {
    try {
      const [nextProfile, nextWorkouts, nextSessions, nextHub, nextRunner] =
        await Promise.all([
          api.profile(),
          api.workouts(),
          api.sessions(),
          api.devicesSnapshot(),
          api.runnerState(),
        ]);
      setProfile(nextProfile);
      setWorkouts(nextWorkouts);
      setSessions(nextSessions);
      setHub(nextHub);
      setRunner(nextRunner);
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
    let offTelemetry: (() => void) | undefined;
    let offRunner: (() => void) | undefined;
    void api.onTelemetry((sample) => {
      setTelemetry(sample);
      setTelemetryHistory((history) => [...history.slice(-239), sample]);
    }).then((off) => {
      offTelemetry = off;
    });
    void api.onRunnerState((state) => {
      setRunner(state);
      if (state.status === "finished") {
        void api.sessions().then(setSessions);
      }
    }).then((off) => {
      offRunner = off;
    });
    // Devices hub: whole-slot updates on state/stats changes, plus individual
    // log lines so the per-device logs grow live between snapshots.
    let offSlot: (() => void) | undefined;
    let offLog: (() => void) | undefined;
    void api.onDeviceSlot((slot) => {
      setHub((current) =>
        current
          ? { ...current, slots: current.slots.map((existing) => (existing.role === slot.role ? slot : existing)) }
          : current,
      );
    }).then((off) => {
      offSlot = off;
    });
    void api.onDeviceLog(({ role, line }) => {
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
    }).then((off) => {
      offLog = off;
    });
    return () => {
      offTelemetry?.();
      offRunner?.();
      offSlot?.();
      offLog?.();
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
    runner.status === "paused" ||
    runner.status === "countdown";

  const perform = async (action: () => Promise<unknown>, label = "user action") => {
    try {
      setError(null);
      await action();
    } catch (cause) {
      const message = messageOf(cause);
      setError(message);
      void api.reportError(label, message).catch(() => undefined);
    }
  };

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
    ["devices", "Devices", Bluetooth],
    ["ride", "Ride", Bike],
    ["history", "History", History],
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
      </aside>

      <main className="content">
        {page === "home" && (
          <Overview
            profile={profile}
            workouts={workouts}
            sessions={sessions}
            connected={connected}
            onConnect={() => setDevicePicker("trainer")}
            onRide={openRide}
            onNavigate={setPage}
          />
        )}
        {page === "devices" && (
          <DevicesPage
            hub={hub}
            sources={telemetry.sources}
            onConnect={setDevicePicker}
            perform={perform}
          />
        )}
        {page === "workouts" && (
          <WorkoutLibrary
            workouts={workouts}
            ftp={profile.ftpWatts}
            onCreate={() => setEditor(newWorkout())}
            onEdit={setEditor}
            onRide={openRide}
            onDelete={(workout) =>
              void perform(async () => {
                await api.deleteWorkout(workout.id);
                setWorkouts(await api.workouts());
              }, "delete workout")
            }
            onExport={(workout) => void perform(() => api.exportZwo(workout), "export zwo")}
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
            distanceUnit={profile.distanceUnit}
            perform={perform}
          />
        )}
        {page === "history" && (
          <HistoryPage
            sessions={sessions}
            selected={selectedSession}
            distanceUnit={profile.distanceUnit}
            onSelect={(session) =>
              void perform(async () =>
                setSelectedSession(await api.session(session.id)),
              "open session")
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
            onSave={(next) =>
              void perform(async () => {
                await api.saveProfile(next);
                setProfile(next);
              }, "save profile")
            }
            onForgetDevices={() => perform(() => api.forgetAllDevices(), "forget all devices")}
          />
        )}
      </main>

      {error && (
        <div className="toast error-toast">
          <span>{error}</span>
          <button onClick={() => setError(null)} aria-label="Dismiss error"><X size={17} /></button>
        </div>
      )}
      {devicePicker && (
        <DevicePicker
          role={devicePicker}
          slot={hub?.slots.find((slot) => slot.role === devicePicker)}
          scanError={hub?.scanError ?? null}
          close={() => setDevicePicker(null)}
          perform={perform}
        />
      )}
      {editor && (
        <WorkoutEditor
          initial={editor}
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
  workouts,
  sessions,
  connected,
  onConnect,
  onRide,
  onNavigate,
}: {
  profile: Profile;
  workouts: Workout[];
  sessions: SessionSummary[];
  connected: boolean;
  onConnect: () => void;
  onRide: (id: string) => void;
  onNavigate: (page: Page) => void;
}) {
  const latest = sessions[0];
  return (
    <>
      <PageHeader eyebrow="GOOD EVENING" title={`Ready to ride, ${profile.name}?`} />
      <section className="hero-grid">
        <article className="card connection-card">
          <div className="card-icon"><Bluetooth /></div>
          <div><span className="label">SMART TRAINER</span><h2>{connected ? "Trainer ready" : "Connect your trainer"}</h2>
            <p>{connected ? "FTMS control is available. Choose a workout when you’re ready." : "Pair an FTMS Bluetooth trainer or use the built-in simulator."}</p>
          </div>
          <button className={connected ? "secondary" : "primary"} onClick={onConnect}>{connected ? "Manage" : "Connect"}</button>
        </article>
        <article className="card ftp-card">
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
            <div className={`workout-visual tone-${index % 3}`}><WorkoutBars steps={workout.steps} /></div>
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

function WorkoutLibrary({
  workouts,
  ftp,
  onCreate,
  onEdit,
  onRide,
  onDelete,
  onExport,
  onImport,
}: {
  workouts: Workout[];
  ftp: number;
  onCreate: () => void;
  onEdit: (workout: Workout) => void;
  onRide: (id: string) => void;
  onDelete: (workout: Workout) => void;
  onExport: (workout: Workout) => void;
  onImport: () => void;
}) {
  return (
    <>
      <PageHeader eyebrow={`${workouts.length} SAVED WORKOUTS`} title="Workout library" actions={<>
        <button className="secondary" onClick={onImport}><Upload size={16} /> Import ZWO</button>
        <button className="primary" onClick={onCreate}><Plus size={17} /> New workout</button>
      </>} />
      <div className="library-grid">
        {workouts.map((workout, index) => (
          <article className="card library-card" key={workout.id}>
            <button className="workout-visual-button" onClick={() => onEdit(workout)}>
              <div className={`workout-visual large tone-${index % 3}`}><WorkoutBars steps={workout.steps} /></div>
            </button>
            <div className="library-body">
              <span className="label">{workout.source.toUpperCase()} · {Math.round(workoutDuration(workout.steps) / 60)} MIN</span>
              <h3>{workout.name}</h3>
              <p>{workout.description || "Structured workout"}</p>
              <div className="chips"><span>{workout.steps.length} blocks</span><span>{ftp} W FTP</span></div>
              <div className="card-actions">
                <button className="primary" onClick={() => onRide(workout.id)}><Play size={15} fill="currentColor" /> Ride</button>
                <button className="icon-button" onClick={() => onExport(workout)} title="Export ZWO"><Download size={17} /></button>
                <button className="icon-button danger" onClick={() => onDelete(workout)} title="Delete"><Trash2 size={17} /></button>
              </div>
            </div>
          </article>
        ))}
      </div>
    </>
  );
}

function Ride({
  workouts,
  selectedWorkout,
  setSelectedWorkout,
  connected,
  onConnect,
  runner,
  telemetry,
  telemetryHistory,
  distanceUnit,
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
  distanceUnit: Profile["distanceUnit"];
  perform: (action: () => Promise<unknown>, label?: string) => Promise<void>;
}) {
  const [targetDraft, setTargetDraft] = useState("100");
  const active = runner.status === "running" || runner.status === "paused" || runner.status === "countdown";
  const selected = workouts.find((workout) => workout.id === selectedWorkout);
  const elapsed = runner.status === "running" || runner.status === "paused" ? runner.elapsedSeconds : 0;
  const total = runner.status === "running" || runner.status === "paused" ? runner.totalSeconds : selected ? workoutDuration(selected.steps) : 0;
  const progress = total ? Math.min(100, (elapsed / total) * 100) : 0;
  const manualErg = (runner.status === "running" || runner.status === "paused") && runner.manualErg;
  const openEnded = (runner.status === "running" || runner.status === "paused") && runner.totalSeconds === null;
  const targetPower = runner.status === "running" || runner.status === "paused"
    ? runner.targetPowerWatts
    : telemetry.targetPowerWatts;
  const displayedSpeed = formatSpeed(telemetry.speedKph ?? 0, distanceUnit);

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

  useEffect(() => {
    if (runner.status !== "running" || !runner.manualErg) return;
    const onKeyDown = (event: KeyboardEvent) => {
      const delta = manualPowerDeltaForKey(event.key, event.repeat);
      if (delta === null) return;
      event.preventDefault();
      void perform(() => api.adjustManualPower(delta), "adjust manual power");
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [perform, runner]);

  return (
    <>
      <PageHeader eyebrow={active ? "WORKOUT IN PROGRESS" : "TRAINING ROOM"} title={active && "workoutName" in runner ? runner.workoutName : "Start a ride"} />
      {!connected && <div className="notice"><Bluetooth /><div><strong>No trainer connected</strong><p>Connect a trainer or the simulator to begin.</p></div><button className="primary" onClick={onConnect}>Connect</button></div>}
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
              {workouts.map((workout) => <button key={workout.id} className={selectedWorkout === workout.id ? "selected" : ""} onClick={() => setSelectedWorkout(workout.id)}>
                <div><strong>{workout.name}</strong><span>{formatDuration(workoutDuration(workout.steps))}</span></div><WorkoutBars steps={workout.steps} /></button>)}
            </div>
            <button className="primary start-button" disabled={!connected || !selectedWorkout} onClick={() => selectedWorkout && void perform(() => api.startWorkout(selectedWorkout), "start workout")}><Play fill="currentColor" /> Start workout</button>
          </div>
          <div className="card ride-preview"><span className="label">WORKOUT PREVIEW</span><h2>{selected?.name ?? "Choose a workout"}</h2><p>{selected?.description}</p>{selected && <><div className="preview-chart"><WorkoutBars steps={selected.steps} /></div><div className="preview-stats"><Metric value={formatDuration(workoutDuration(selected.steps))} unit="duration" /><Metric value={`${selected.steps.length}`} unit="blocks" /></div></>}</div>
        </section>
      ) : (
        <section className="live-ride">
          {runner.status === "countdown" && <div className="countdown">{runner.seconds}</div>}
          <div className="metrics-grid">
            <LiveMetric icon={Zap} label="POWER" value={telemetry.powerWatts} unit="W" accent note={sourceNote(telemetry.sources?.power)} />
            <LiveMetric icon={Gauge} label="CADENCE" value={Math.round(telemetry.cadenceRpm ?? 0)} unit="rpm" note={sourceNote(telemetry.sources?.cadence)} />
            <LiveMetric icon={Radio} label="SPEED" value={displayedSpeed.value} unit={displayedSpeed.unit} />
            <LiveMetric icon={HeartPulse} label="HEART RATE" value={telemetry.heartRateBpm ?? "—"} unit="bpm" note={sourceNote(telemetry.sources?.heartRate)} />
          </div>
          <div className="card live-chart">
            <div className={manualErg || openEnded ? "target-line editable" : "target-line"}>
              <span>Target power</span>
              {manualErg || openEnded ? (
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
                        if (event.key === "Enter") {
                          event.currentTarget.blur();
                        }
                      }}
                    />
                    <strong>W</strong>
                  </label>
                  <button className="secondary" disabled={runner.status !== "running"} onClick={() => void adjustPower(5)}>+ 5 W</button>
                </div>
              ) : <strong>{targetPower ?? "Free"}{targetPower !== null ? " W" : ""}</strong>}
              {(manualErg || openEnded) && <span className="manual-erg-hint">Type watts or use ↑ / ↓</span>}
            </div>
            <ResponsiveContainer width="100%" height={220}><AreaChart data={telemetryHistory}><defs><linearGradient id="powerFill" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stopColor="#c8ff32" stopOpacity={0.45}/><stop offset="100%" stopColor="#c8ff32" stopOpacity={0}/></linearGradient></defs><CartesianGrid strokeDasharray="4 4" vertical={false} /><XAxis dataKey="timestampMs" hide /><YAxis width={40} domain={[0, "dataMax + 50"]} /><Tooltip labelFormatter={() => ""} formatter={(value) => [`${value} W`, "Power"]} /><Area type="monotone" dataKey="powerWatts" stroke="#c8ff32" fill="url(#powerFill)" isAnimationActive={false} /></AreaChart></ResponsiveContainer>
            {openEnded
              ? <div className="open-ended-time"><span>Elapsed</span><strong>{formatDuration(elapsed)}</strong><span>Open ended</span></div>
              : <div className="progress-meta"><span>{formatDuration(elapsed)}</span><div className="progress"><i style={{ width: `${progress}%` }} /></div><span>-{formatDuration(Math.max(0, (total ?? 0) - elapsed))}</span></div>}
          </div>
          <div className="ride-controls">
            <button className="secondary control" onClick={() => void perform(() => api.pauseOrResume(), "pause/resume")}>{runner.status === "paused" ? <Play /> : <Pause />} {runner.status === "paused" ? "Resume" : "Pause"}</button>
            {!openEnded && <button className="secondary control" onClick={() => void perform(() => api.skipInterval(), "skip interval")}><SkipForward /> Skip</button>}
            <button className="stop control" onClick={() => void perform(() => api.stopWorkout(), "stop workout")}><CircleStop /> End ride</button>
          </div>
        </section>
      )}
      {runner.status === "finished" && <div className="toast success-toast">Ride saved to history.</div>}
    </>
  );
}

function HistoryPage({ sessions, selected, distanceUnit, onSelect, onClose, onExport, onExportFit, onGarmin }: { sessions: SessionSummary[]; selected: SessionDetail | null; distanceUnit: Profile["distanceUnit"]; onSelect: (session: SessionSummary) => void; onClose: () => void; onExport: (session: SessionSummary) => void; onExportFit: (session: SessionSummary) => void; onGarmin: (session: SessionSummary) => void }) {
  return (
    <>
      <PageHeader eyebrow={`${sessions.length} RECORDED RIDES`} title="Ride history" />
      {sessions.length === 0 ? <div className="empty-state"><History /><h2>No rides yet</h2><p>Completed and stopped workouts appear here automatically.</p></div> :
      <div className="history-list">{sessions.map((session) => <button key={session.id} className="history-row" onClick={() => onSelect(session)}>
        <span className={session.completed ? "completion complete" : "completion"}>{session.completed ? "✓" : "–"}</span>
        <div className="history-title"><strong>{session.workoutName}</strong><span>{new Date(session.startedAt).toLocaleString()}</span></div>
        <Metric value={formatDuration(session.elapsedSeconds)} unit="duration" /><Metric value={`${session.averagePowerWatts}`} unit="W avg" /><DistanceMetric meters={session.estimatedDistanceMeters} unit={distanceUnit} /><ChevronRight />
      </button>)}</div>}
      {selected && <div className="modal-backdrop"><div className="modal detail-modal"><button className="modal-close" onClick={onClose}><X /></button><span className="label">RIDE DETAIL</span><h2>{selected.summary.workoutName}</h2><p>{new Date(selected.summary.startedAt).toLocaleString()}</p><div className="detail-metrics"><Metric value={formatDuration(selected.summary.elapsedSeconds)} unit="duration" /><Metric value={`${selected.summary.averagePowerWatts}`} unit="W average" /><Metric value={`${selected.summary.maxPowerWatts}`} unit="W maximum" /><Metric value={`${Math.round(selected.summary.averageCadenceRpm ?? 0)}`} unit="rpm average" /><DistanceMetric meters={selected.summary.estimatedDistanceMeters} unit={distanceUnit} /></div><p className="distance-note">Estimated distance · {distanceSourceLabel(selected.summary.distanceSource)}</p><ResponsiveContainer width="100%" height={220}><AreaChart data={selected.samples}><CartesianGrid strokeDasharray="4 4" vertical={false}/><XAxis dataKey="timestampMs" hide/><YAxis width={42}/><Tooltip labelFormatter={() => ""}/><Area type="monotone" dataKey="powerWatts" stroke="#c8ff32" fill="#c8ff3233" isAnimationActive={false}/></AreaChart></ResponsiveContainer><div className="detail-actions"><button className="primary" onClick={() => onGarmin(selected.summary)}><Upload size={16}/> Upload to Garmin</button><button className="secondary" onClick={() => onExportFit(selected.summary)}><Download size={16}/> Export FIT</button><button className="secondary" onClick={() => onExport(selected.summary)}><Download size={16}/> Export CSV</button></div><p className="handoff-note">Garmin Connect and Finder will open. Drag the selected FIT file onto Garmin’s import page, then confirm the upload.</p></div></div>}
    </>
  );
}

const KG_PER_LB = 0.45359237;

function displayedWeight(kg: number, unit: Profile["weightUnit"]) {
  return Number((unit === "lb" ? kg / KG_PER_LB : kg).toFixed(1));
}

function storedWeight(value: number, unit: Profile["weightUnit"]) {
  return unit === "lb" ? value * KG_PER_LB : value;
}

function SettingsPage({ profile, onSave, onForgetDevices }: { profile: Profile; onSave: (profile: Profile) => void; onForgetDevices: () => Promise<void> }) {
  const [draft, setDraft] = useState(profile);
  const [logPath, setLogPath] = useState("Loading log location…");
  const [rideFilesPath, setRideFilesPath] = useState("Loading ride files location…");
  useEffect(() => {
    void api.logFilePath().then(setLogPath);
    void api.rideFilesPath().then(setRideFilesPath);
  }, []);
  useEffect(() => setDraft(profile), [profile]);
  return (
    <>
      <PageHeader eyebrow="LOCAL PROFILE" title="Settings" />
      <section className="card settings-card"><div><span className="label">RIDER PROFILE</span><h2>Training and distance</h2><p>Your weight and bike weight support flat-road distance estimates when the trainer does not report speed. Values are stored in kilograms regardless of display units.</p></div><form onSubmit={(event) => { event.preventDefault(); onSave(draft); }}>
        <label>Rider name<input value={draft.name} onChange={(event) => setDraft({ ...draft, name: event.target.value })}/></label>
        <div className="form-row"><label>FTP (watts)<input type="number" min="50" max="500" value={draft.ftpWatts} onChange={(event) => setDraft({ ...draft, ftpWatts: Number(event.target.value) })}/></label><label>Safety power limit<input type="number" min="100" max="2500" value={draft.maxPowerWatts} onChange={(event) => setDraft({ ...draft, maxPowerWatts: Number(event.target.value) })}/></label></div>
        <div className="form-row"><label>Weight unit<select value={draft.weightUnit} onChange={(event) => setDraft({ ...draft, weightUnit: event.target.value as Profile["weightUnit"] })}><option value="kg">Kilograms (kg)</option><option value="lb">Pounds (lb)</option></select></label><label>Distance unit<select value={draft.distanceUnit} onChange={(event) => setDraft({ ...draft, distanceUnit: event.target.value as Profile["distanceUnit"] })}><option value="km">Kilometers</option><option value="mi">Miles</option></select></label></div>
        <div className="form-row"><label>Rider weight ({draft.weightUnit})<input type="number" step="0.1" min={draft.weightUnit === "lb" ? 66 : 30} max={draft.weightUnit === "lb" ? 551 : 250} value={displayedWeight(draft.riderWeightKg, draft.weightUnit)} onChange={(event) => setDraft({ ...draft, riderWeightKg: storedWeight(Number(event.target.value), draft.weightUnit) })}/></label><label>Bike weight ({draft.weightUnit})<input type="number" step="0.1" min={draft.weightUnit === "lb" ? 7 : 3} max={draft.weightUnit === "lb" ? 88 : 40} value={displayedWeight(draft.bikeWeightKg, draft.weightUnit)} onChange={(event) => setDraft({ ...draft, bikeWeightKg: storedWeight(Number(event.target.value), draft.weightUnit) })}/></label></div>
        <button className="primary" type="submit">Save settings</button>
      </form></section>
      <section className="card settings-card"><div><span className="label">DATA & DIAGNOSTICS</span><h2>Local-first by design</h2><p>Every finalized ride is stored in SQLite and as a persistent Garmin-compatible FIT file. Missing FIT files are regenerated automatically.</p></div><div className="data-locations"><div className="log-location"><span>Ride Files</span><code>{rideFilesPath}</code><button className="secondary" onClick={() => void api.revealRideFiles().catch(() => undefined)}>Show Ride Files</button></div><div className="log-location"><span>Log file</span><code>{logPath}</code><button className="secondary" onClick={() => void api.revealLogFile().catch(() => undefined)}>Show in folder</button><button className="secondary" onClick={() => void navigator.clipboard.writeText(logPath)}>Copy path</button></div><div className="log-location"><span>Known devices</span><p className="settings-note">Devices you have connected are remembered on this computer so they can be reconnected without scanning. Forgetting them does not disconnect anything.</p><button className="danger-button" onClick={() => void onForgetDevices()}>Forget all devices</button></div></div></section>
    </>
  );
}

function WorkoutEditor({ initial, close, save }: { initial: Workout; close: () => void; save: (workout: Workout) => void }) {
  const [workout, setWorkout] = useState(structuredClone(initial));
  const updateStep = (index: number, patch: Partial<WorkoutStep>) => setWorkout({ ...workout, steps: workout.steps.map((step, stepIndex) => stepIndex === index ? { ...step, ...patch } as WorkoutStep : step) });
  const addStep = (kind: "steady" | "ramp" | "freeRide") => {
    const step: WorkoutStep = kind === "steady" ? { kind, durationSeconds: 300, target: { unit: "percentFtp", value: 75 } } : kind === "ramp" ? { kind, durationSeconds: 300, start: { unit: "percentFtp", value: 50 }, end: { unit: "percentFtp", value: 90 } } : { kind, durationSeconds: 300 };
    setWorkout({ ...workout, steps: [...workout.steps, step] });
  };
  return <div className="modal-backdrop"><div className="modal editor-modal"><button className="modal-close" onClick={close}><X /></button><span className="label">WORKOUT BUILDER</span><div className="editor-title"><input value={workout.name} onChange={(event) => setWorkout({ ...workout, name: event.target.value })}/><strong>{formatDuration(workoutDuration(workout.steps))}</strong></div><textarea placeholder="Workout description" value={workout.description} onChange={(event) => setWorkout({ ...workout, description: event.target.value })}/>
    <div className="step-list">{workout.steps.map((step, index) => <div className="step-editor" key={`${index}-${step.kind}`}><span className={`step-kind ${step.kind}`}>{step.kind === "freeRide" ? "FREE" : step.kind.toUpperCase()}</span><label>Duration (sec)<input type="number" min="1" value={step.kind === "repeat" ? workoutDuration(step.steps) : step.durationSeconds} disabled={step.kind === "repeat"} onChange={(event) => updateStep(index, { durationSeconds: Number(event.target.value) } as Partial<WorkoutStep>)}/></label>{step.kind === "steady" && <TargetInput label="Power (% FTP)" target={step.target} onChange={(target) => updateStep(index, { target })}/>} {step.kind === "ramp" && <><TargetInput label="Start (% FTP)" target={step.start} onChange={(start) => updateStep(index, { start })}/><TargetInput label="End (% FTP)" target={step.end} onChange={(end) => updateStep(index, { end })}/></>} {step.kind === "repeat" && <span className="repeat-summary">{step.repetitions}× repeat group</span>}<button className="icon-button danger" onClick={() => setWorkout({ ...workout, steps: workout.steps.filter((_, stepIndex) => stepIndex !== index) })}><Trash2 size={16}/></button></div>)}</div>
    <div className="add-steps"><span>Add block</span><button onClick={() => addStep("steady")}><Plus/>Steady</button><button onClick={() => addStep("ramp")}><Plus/>Ramp</button><button onClick={() => addStep("freeRide")}><Plus/>Free ride</button></div>
    <div className="editor-actions"><button className="secondary" onClick={close}>Cancel</button><button className="primary" disabled={!workout.name.trim() || workout.steps.length === 0} onClick={() => save(workout)}>Save workout</button></div>
  </div></div>;
}

function TargetInput({ label, target, onChange }: { label: string; target: { unit: "watts" | "percentFtp"; value: number }; onChange: (target: { unit: "watts" | "percentFtp"; value: number }) => void }) {
  return <label>{label}<input type="number" min="1" max="300" value={target.unit === "percentFtp" ? target.value : target.value} onChange={(event) => onChange({ unit: "percentFtp", value: Number(event.target.value) })}/></label>;
}

function WorkoutBars({ steps }: { steps: WorkoutStep[] }) {
  const flattened = useMemo(() => flattenSteps(steps), [steps]);
  const total = workoutDuration(flattened);
  return <div className="workout-bars">{flattened.map((step, index) => {
    const target = step.kind === "steady" ? step.target.value : step.kind === "ramp" ? Math.max(step.start.value, step.end.value) : 40;
    const duration = step.kind === "repeat" ? 0 : step.durationSeconds;
    return <i key={index} style={{ width: `${Math.max(3, duration / total * 100)}%`, height: `${Math.min(100, Math.max(20, target / 1.4))}%` }} />;
  })}</div>;
}

function flattenSteps(steps: WorkoutStep[]): WorkoutStep[] {
  return steps.flatMap((step) => step.kind === "repeat" ? Array.from({ length: step.repetitions }, () => flattenSteps(step.steps)).flat() : [step]);
}

function Metric({ value, unit }: { value: string; unit: string }) {
  return <div className="metric"><strong>{value}</strong><span>{unit}</span></div>;
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

function LiveMetric({ icon: Icon, label, value, unit, accent = false, note }: { icon: typeof Activity; label: string; value: string | number; unit: string; accent?: boolean; note?: string | null }) {
  return <div className={accent ? "live-metric accent" : "live-metric"}><span><Icon size={17}/>{label}</span><strong>{value}<small>{unit}</small></strong>{note && <em className="metric-source">{note}</em>}</div>;
}

function newWorkout(): Workout {
  const now = new Date().toISOString();
  return { id: crypto.randomUUID(), name: "New workout", description: "", source: "local", version: 1, createdAt: now, updatedAt: now, steps: [{ kind: "steady", durationSeconds: 300, target: { unit: "percentFtp", value: 70 } }] };
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
