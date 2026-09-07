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
  yaw_deg: number
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

export interface FleetSnapshot {
  phase: string // 'INIT' | 'RUNNING' | 'ABORTED' | ...
  t_s: number
  vehicles: FleetVehicle[]
  tasks: FleetTask[]
  geofence: Geofence
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
