/**
 * Shared types for the rustsitsim / mavfleet operator console.
 *
 * Schemas follow:
 *   - rustsitsim/docs/SPEC.md §4 (control/telemetry plane, WS frame v0.1 at 10 Hz)
 *   - mavfleet/docs/SPEC.md §3.4 (fleet REST/WS plane) + §5 (VehicleState, event log)
 *
 * All structs are written defensively: the Rust backends may come online with
 * slightly different envelope nesting, so normalizers in `conn.ts` map onto
 * these canonical shapes.
 */

/** Connection state of a backend plane. */
export type ConnState = 'connecting' | 'live' | 'simulated'

/** rustsitsim run phase. */
export type SimPhase = 'WAIT' | 'RUN' | 'DONE' | 'ABORT' | string

/** mavfleet vehicle FSM states (SPEC §4). */
export type FsmState =
  | 'INIT'
  | 'SPAWNING'
  | 'BOOTING'
  | 'READY'
  | 'ACTIVE'
  | 'RTL'
  | 'LANDED'
  | 'FAULT'
  | (string & {})

/** Event severity for the fleet event log. */
export type EventSeverity = 'info' | 'warn' | 'critical'

// ---------------------------------------------------------------------------
// rustsitsim frames
// ---------------------------------------------------------------------------

export interface SimFrameState {
  pos_ned_m: [number, number, number]
  vel_ned_ms: [number, number, number]
  q_wxyz: [number, number, number, number]
  omega_body_rads: [number, number, number]
  motors: number[]
  battery_pct: number
}

export interface SimFrameSensors {
  accel_ms2: [number, number, number]
  gyro_rads: [number, number, number]
  baro_alt_m: number
  gps_fix: number
  gps_sat: number
}

export interface ActiveFault {
  id: string
  type: string
  /** Free-form parameter echo (motor, factor, duration_s, ...). */
  params: Record<string, number | string>
  /** Wall-clock ms when the fault was injected/cleared-detected. */
  since: number
  /** Seconds until auto-clear; null = persistent. */
  ttl_s: number | null
}

export interface SimFrame {
  t_us: number
  phase: SimPhase
  state: SimFrameState
  sensors: SimFrameSensors
  faults_active: ActiveFault[]
  stats: {
    tick_p95_us: number | null
    sent: Record<string, number>
    recv: Record<string, number>
  }
}

export interface SimStatus {
  phase: SimPhase
  px4_connected: boolean
  loop_closed: boolean
  tick_p95_us: number | null
  rate_hz: number | null
  t_us: number
}

/** One position sample for the trajectory plot. */
export interface PosSample {
  t: number // s, virtual
  n: number
  e: number
  d: number
  yaw_deg: number
}

// ---------------------------------------------------------------------------
// mavfleet snapshots
// ---------------------------------------------------------------------------

export type TaskStatus = 'pending' | 'assigned' | 'in_progress' | 'done' | 'rejected'

export interface FleetVehicle {
  id: string
  index: number
  sysid: number
  mode: string
  fsm: FsmState
  battery_pct: number
  voltage_v: number | null
  position_ned_m: [number, number, number]
  velocity_ned_ms: [number, number, number]
  /** ADR-0017: PX4's GLOBAL_POSITION_INT fix (null before the first fix
   * arrives) — the geo map renders the estimate the vehicle flies. */
  lat: number | null
  lon: number | null
  /** MSL altitude / AGL-relative altitude, metres (null = no fix). */
  alt_msl_m: number | null
  alt_agl_m: number | null
  /** HEARTBEAT's MAV_MODE_FLAG_SAFETY_ARMED (the command bar's gate). */
  armed: boolean
  yaw_deg: number
  /**
   * Attitude quaternion in ZYX NED aerospace convention, `[w, x, y, z]`.
   *
   * P5 (GCS v2, GCS_V2_SPEC.md §8.3): the wire HAS this field (live wire:
   * `vehicles[].attitude_q_wxyz`; v1's `conn.ts:301` probed
   * `q_wxyz` / `attitude.q_wxyz` and missed it, so v1's FlyView HUD
   * synthesized a fake quaternion from `yaw_deg` alone — roll/pitch = 0).
   *
   * Optional for v1 backward compatibility (mock engines and older
   * backends omit it); the v2 `lib/normalize.ts` always populates it when
   * present on the wire. The v2 FlyView / Attitude HUD reads it directly
   * via `lib/format.ts:quatToEulerDeg`; `null` = not on the wire, fall back
   * to the v1 yaw-only synthesis.
   */
  attitude_q_wxyz?: [number, number, number, number] | null
  heartbeat_age_s: number
  stale: boolean
  health: string[]
  task_id: string | null
  breadcrumb: { n: number; e: number }[]
}

export interface FleetTask {
  id: string
  pos_ned_m: [number, number, number]
  hover_s: number
  reward: number
  status: TaskStatus
  assigned_to: string | null
  /** Progress 0..1 while in flight (fraction of route flown). */
  progress: number
}

export interface FleetEvent {
  /** Wall-clock epoch ms. */
  t: number
  /** Virtual time seconds. */
  t_s: number
  kind: string // 'fsm' | 'supervisor' | 'fault' | 'award' | 'link' | 'boundary' | 'run'
  vehicle: string | null
  detail: string
  severity: EventSeverity
}

export interface AuctionEntry {
  t: number
  round: number
  task: string
  vehicle: string
  bid_s: number
  bidders: number
}

export interface Geofence {
  points: [number, number][] // [north, east] pairs
  ceiling_m: number
  floor_m: number
}

/** The geodetic anchor of the NED frame (ADR-0017 `[env] origin`). */
export interface GeoOriginView {
  lat_deg: number
  lon_deg: number
  alt_m: number
}

export interface FleetSnapshot {
  phase: string // 'INIT' | 'RUNNING' | 'ABORTED' | ...
  t_s: number
  vehicles: FleetVehicle[]
  tasks: FleetTask[]
  geofence: Geofence
  geo_origin: GeoOriginView | null
}

// ---------------------------------------------------------------------------
// Operator map control (mavfleet ADR-0017 — the QGC Fly/Plan workflow)
// ---------------------------------------------------------------------------

/** One operator waypoint being planned on the map (QGC Plan View item). */
export interface MapWaypoint {
  /** Local sequence id (wp1, wp2, ...) — backend assigns op* task ids. */
  key: string
  lat: number
  lng: number
  /** Metres AGL (QGC's waypoint altitude convention). */
  alt_m: number
  hover_s: number
}

/** Upload result from POST /api/mission. */
export interface MissionUploadResult {
  accepted: string[]
  rejected: { label: string; reason: string }[]
  pool: number
}

/** Generic guided-command result (arm/takeoff/land/rtl/hold/goto). */
export interface GuidedResult {
  command: string
  result: number
  accepted: boolean
  index?: number
  [k: string]: unknown
}

// ---------------------------------------------------------------------------
// Vehicle Setup (mavfleet ADR-0016 — the QGC/Mission-Planner workflow)
// ---------------------------------------------------------------------------

/** One PX4 airframe from the ROMFS-derived catalog (GET /api/airframes). */
export interface Airframe {
  /** SYS_AUTOSTART value that selects this airframe at boot. */
  id: number
  name: string
  frame_type: string
  /** SITL model tag (posix airframes), null for real-vehicle frames. */
  sim_model: string | null
}

/** Airframe catalog group (QGC browser order). */
export interface AirframeGroup {
  category: string // 'Multirotor (UAV)' | 'Boat (USV)' | ...
  airframes: Airframe[]
}

/** One switchable flight mode (GET /api/modes). */
export interface ModeEntry {
  name: string
  mode_word: number
}

/** Param-download state (fleet-mavlink ParamDownloadState). */
export type ParamDownloadState = 'Idle' | 'Downloading' | 'Complete' | 'Stalled' | 'NoLink' | (string & {})

/** One parameter row in the live cache (GET /api/vehicles/{i}/params).
 *
 * v1 (GCS_SPEC §5.3) extends this row with the PX4-default comparison
 * fields so the operator can diff the live vehicle against the
 * compiled-in defaults: `group` (PX4 param-group prefix), `default`
 * (null = unknown; the backend may not have the catalog default for
 * every param), and `is_changed` (the row's diff flag, the UX basis
 * for the pale-yellow highlight). `raw` + `kind` preserve the wire
 * shape (MAVLink PARAM_VALUE's `param_value` is a tagged union; the
 * server emits both the decoded `value` and the raw form). */
export interface ParamEntry {
  id: string
  value: number
  raw: number
  type: number
  kind: string
  index: number
  group: string
  default: number | null
  is_changed: boolean
}

/** The parameter store snapshot with download progress. */
export interface ParamStoreView {
  total: number
  received: number
  state: ParamDownloadState
  requested_ms: number | null
  last_value_ms: number | null
  params: ParamEntry[]
}

/** One preset summary row (GET /api/vehicles/{i}/param-presets — :8300). */
export interface PresetSummary {
  name: string
  created_at: string
  param_count: number
}

/** One parameter inside a saved preset (POST body / load response). */
export interface PresetParam {
  id: string
  value: number
  type: number
}

/** Full preset file (saved on the catalog side). */
export interface PresetFile {
  name: string
  created_at: string
  vehicle_id: number
  params: PresetParam[]
}

/** Airframe as resolved against the live SYS_AUTOSTART (setup summary). */
export interface AirframeResolved {
  sys_autostart: number | null
  name: string
  frame_type: string
  category: string
  /** True when this stack's quad-rotor HIL dynamics can fly it. */
  dynamics_compatible: boolean
}

/** Sensor calibration flags (CAL_*_ID nonzero = calibrated, QGC's rule). */
export interface CalibrationView {
  accel: boolean | null
  gyro: boolean | null
  mag0: boolean | null
  mag1: boolean | null
  mag2: boolean | null
  level_horizon: boolean | null
}

/** GET /api/vehicles/{i}/setup — the QGC setup Summary. */
export interface VehicleSetupSummary {
  index: number
  sysid: number
  compid: number
  fsm: string
  mode: string
  mode_word: number
  armed: boolean
  battery_pct: number
  voltage_v: number | null
  restart_pending: boolean
  autopilot: { type: string; version: string }
  airframe: AirframeResolved
  params: { total: number; received: number; state: ParamDownloadState } | null
  calibration: CalibrationView | null
  power: Record<string, number | null> | null
  safety: Record<string, number | null> | null
}

/** Sensor ids the calibrate endpoint accepts (MAV_CMD 241 matrix). */
export type CalSensor = 'gyro' | 'accel' | 'accel_quick' | 'mag' | 'level' | 'baro' | 'airspeed'

// ---------------------------------------------------------------------------
// Fleet mission bindings + orchestration (GCS_SPEC §5.4 / §8.4)
// ---------------------------------------------------------------------------

/** Per-vehicle mission binding state machine (SPEC §5.4 AC-5.4.1). */
export type MissionBindingState =
  | 'unassigned'
  | 'assigned'
  | 'uploaded'
  | 'active'
  | 'complete'
  | 'aborted'

/** One row in GET /api/fleet/mission-bindings (:8400). */
export interface MissionBinding {
  vehicle_id: number
  mission_id: string
  binding_state: MissionBindingState
}

/** Body of POST /api/fleet/mission-bindings. */
export interface MissionBindingInput {
  vehicle_id: number
  mission_id: string
}

/**
 * Per-vehicle result of POST /api/fleet/start. The fleet orchestrator walks
 * every bound vehicle and emits one of these rows; the UI surfaces them as
 * they arrive (the modal closes immediately, the result table streams in).
 */
export interface FleetStartResult {
  vehicle_id: number
  mission_id: string
  status: 'started' | 'failed' | 'timeout'
  /** Human-readable detail (error message / gate reason). */
  detail?: string
}

/** Orchestration mode for POST /api/fleet/start. */
export type FleetStartMode = 'parallel' | 'sequential'

/** Sequential gate condition (SPEC §5.4 AC-5.4.2). */
export type SequentialGate = 'first_waypoint' | 'takeoff_complete'

/** Body of POST /api/fleet/start. */
export interface FleetStartRequest {
  mode: FleetStartMode
  sequential_gate?: SequentialGate
  timeout_s?: number
}

/** One swarming pattern in the library (GET /api/fleet/patterns). */
export interface SwarmPattern {
  name: string
  description: string
}

/**
 * One per-vehicle mission emitted by POST /api/fleet/patterns/{name}/generate.
 * The shape mirrors PlanMissionSummary so the operator can upload it directly
 * via the existing catalog upload path (§5.1).
 */
export interface GeneratedPatternMission {
  vehicle_id: number
  mission_id: string
  waypoint_count: number
  /** Optional preview waypoints (NED) for the map overlay. */
  preview_ned_m?: [number, number, number][]
}

/** Result of POST /api/fleet/patterns/{name}/generate. */
export interface PatternGenerationResult {
  pattern: string
  missions: GeneratedPatternMission[]
}

// ---------------------------------------------------------------------------
// Analyze View — replay/ULog browse + scrub + plot (GCS_SPEC §5.5 / §8.5)
// ---------------------------------------------------------------------------

/** One row of GET /api/replays (GCS_SPEC §5.5 — RustSim's .replay files). */
export interface ReplayFile {
  filename: string
  duration_s: number
  vehicle_count: number
  scenario_sha256: string
  mtime: number
}

/** GET /api/replays/{file}/meta — header info (tick rate, seed, scenario hash). */
export interface ReplayMeta {
  tick_rate_hz: number
  seed: number | string
  scenario_sha256: string
  records: number
  virtual_duration_s: number
  final_pos_ned_m?: [number, number, number]
  final_q_wxyz?: [number, number, number, number]
  /** Optional geo anchor (some replays carry the scenario origin). */
  geo_origin?: { lat_deg: number; lon_deg: number; alt_m: number } | null
}

/** GET /api/replays/{file}/data?from_tick&to_tick&topic — tick-aligned samples.
 * Values may be scalars (battery_pct) or [n,e,d] / [w,x,y,z] vectors. */
export interface ReplayTopicData {
  ticks: number[]
  values: number[] | number[][]
}

/** One row of GET /api/ulogs (PX4 .ulg files). */
export interface UlogFile {
  filename: string
  size_bytes: number
  mtime: number
}

/** GET /api/ulogs/{file}/topics/{topic}/data?from_s&to_s — topic field data. */
export interface UlogTopicData {
  t: number[]
  fields: Record<string, number[]>
}

/** Whether the loaded artifact is a RustSim replay or a PX4 ULog. */
export type AnalyzeSourceKind = 'replay' | 'ulog'

/** Discriminated union for the loaded source. */
export interface AnalyzeSelection {
  kind: AnalyzeSourceKind
  filename: string
}

/** One configured plot in the strip-chart panel. */
export interface AnalyzePlotConfig {
  /** Local UUID so the operator can stack multiple plots of the same topic. */
  id: string
  topic: string
  /** ULog only — which field to plot. Replays use the topic itself. */
  field?: string
  color: string
}

// ---------------------------------------------------------------------------
// Fault injection catalog (rustsitsim SPEC §7.1)
// ---------------------------------------------------------------------------

export interface FaultFieldSpec {
  key: string
  label: string
  kind: 'number' | 'select'
  min?: number
  max?: number
  step?: number
  unit?: string
  options?: { value: string; label: string }[]
  defaultValue: number | string
}

export interface FaultDef {
  type: string
  code: string
  label: string
  effect: string
  fields: FaultFieldSpec[]
}

export const FAULT_CATALOG: FaultDef[] = [
  {
    type: 'motor_efficiency',
    code: 'F-01',
    label: 'Motor efficiency',
    effect: 'Multiplies u_i — yaw drift, altitude sag',
    fields: [
      { key: 'motor', label: 'Motor', kind: 'select', defaultValue: '0', options: MOTOR_OPTIONS() },
      { key: 'factor', label: 'Factor', kind: 'number', min: 0, max: 1, step: 0.05, defaultValue: 0.55 },
    ],
  },
  {
    type: 'motor_cut',
    code: 'F-02',
    label: 'Motor cut',
    effect: 'Sets u_i to 0 with rotor wind-down',
    fields: [{ key: 'motor', label: 'Motor', kind: 'select', defaultValue: '1', options: MOTOR_OPTIONS() }],
  },
  {
    type: 'imu_bias',
    code: 'F-03',
    label: 'IMU bias ramp',
    effect: 'Ramping bias on gyro/accel output',
    fields: [
      { key: 'sensor', label: 'Sensor', kind: 'select', defaultValue: 'gyro', options: [
        { value: 'gyro', label: 'Gyro' }, { value: 'accel', label: 'Accel' },
      ] },
      { key: 'axis', label: 'Axis', kind: 'select', defaultValue: 'x', options: AXIS_OPTIONS() },
      { key: 'rate', label: 'Ramp rate', kind: 'number', min: 0.001, max: 0.5, step: 0.005, unit: '/s', defaultValue: 0.02 },
    ],
  },
  {
    type: 'imu_saturation',
    code: 'F-04',
    label: 'IMU saturation',
    effect: 'Clamps the sensor output — stuck reading',
    fields: [
      { key: 'sensor', label: 'Sensor', kind: 'select', defaultValue: 'accel', options: [
        { value: 'gyro', label: 'Gyro' }, { value: 'accel', label: 'Accel' },
      ] },
      { key: 'axis', label: 'Axis', kind: 'select', defaultValue: 'z', options: AXIS_OPTIONS() },
      { key: 'limit', label: 'Clamp limit', kind: 'number', min: 0.1, max: 50, step: 0.1, defaultValue: 0.5 },
    ],
  },
  {
    type: 'gps_denial',
    code: 'F-05',
    label: 'GPS denial',
    effect: 'Suppresses fixes, ramps eph to clamp',
    fields: [{ key: 'duration_s', label: 'Duration', kind: 'number', min: 1, max: 300, step: 1, unit: 's', defaultValue: 20 }],
  },
  {
    type: 'gps_glitch',
    code: 'F-06',
    label: 'GPS glitch',
    effect: 'Constant offset on reported position',
    fields: [
      { key: 'offset_m', label: 'Offset', kind: 'number', min: 1, max: 100, step: 1, unit: 'm', defaultValue: 8 },
      { key: 'duration_s', label: 'Duration', kind: 'number', min: 1, max: 300, step: 1, unit: 's', defaultValue: 15 },
    ],
  },
  {
    type: 'baro_drift',
    code: 'F-07',
    label: 'Baro drift',
    effect: 'Ramps pressure-altitude bias',
    fields: [{ key: 'rate_m_s', label: 'Drift rate', kind: 'number', min: 0.01, max: 2, step: 0.01, unit: 'm/s', defaultValue: 0.1 }],
  },
  {
    type: 'wind_event',
    code: 'F-08',
    label: 'Wind event',
    effect: 'Adds gust to steady wind',
    fields: [
      { key: 'wind_n', label: 'Wind N', kind: 'number', min: -15, max: 15, step: 0.5, unit: 'm/s', defaultValue: 5 },
      { key: 'wind_e', label: 'Wind E', kind: 'number', min: -15, max: 15, step: 0.5, unit: 'm/s', defaultValue: -3 },
      { key: 'rise_s', label: 'Rise time', kind: 'number', min: 0.5, max: 20, step: 0.5, unit: 's', defaultValue: 3 },
    ],
  },
  {
    type: 'transport_delay',
    code: 'F-09',
    label: 'Transport delay',
    effect: 'Delays HIL frames on the wire',
    fields: [{ key: 'ms', label: 'Delay', kind: 'number', min: 1, max: 500, step: 1, unit: 'ms', defaultValue: 40 }],
  },
  {
    type: 'packet_drop',
    code: 'F-10',
    label: 'Packet drop',
    effect: 'Drops outgoing sensor frames',
    fields: [{ key: 'percent', label: 'Drop rate', kind: 'number', min: 0, max: 100, step: 1, unit: '%', defaultValue: 15 }],
  },
]

function MOTOR_OPTIONS() {
  return [0, 1, 2, 3].map((m) => ({ value: String(m), label: `Motor ${m}` }))
}

function AXIS_OPTIONS() {
  return [
    { value: 'x', label: 'X' },
    { value: 'y', label: 'Y' },
    { value: 'z', label: 'Z' },
  ]
}
