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

// ---------------------------------------------------------------------------
// Analyze View — ULog browse + scrub + plot (GCS_SPEC §5.5 / §8.5)
// ---------------------------------------------------------------------------

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

/** Whether the loaded artifact is a PX4 ULog. */
export type AnalyzeSourceKind = 'ulog'

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
  /** ULog only — which field to plot. */
  field?: string
  color: string
}

