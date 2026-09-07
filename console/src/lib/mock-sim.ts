/**
 * Client-side stand-in for the rustsitsim telemetry plane (SPEC §4).
 *
 * Used when the Rust backend on :8200 is unreachable (sandbox / dev). Produces
 * the same 10 Hz WS frame schema as /ws/telemetry: a drone flying a waypoint
 * square with battery drain, motor outputs, noisy sensors, and runtime fault
 * effects that respond to inject / clear / estop exactly like the real plane.
 */

import { gaussian, mulberry32, clamp } from './format'
import type { ActiveFault, SimFrame, SimStatus } from './types'

const G = 9.81

interface MockFault {
  af: ActiveFault
  effect: {
    kind: 'gps_denial' | 'motor_eff' | 'motor_cut' | 'baro_drift' | 'wind' | 'imu_bias' | 'imu_sat' | 'gps_glitch' | 'delay' | 'drop'
    motor?: number
    factor?: number
    rate_m_s?: number
    windN?: number
    windE?: number
    riseT?: number
    sensor?: 'gyro' | 'accel'
    axis?: 0 | 1 | 2
    rate?: number
    limit?: number
  }
}

type Phase = 'WAIT' | 'RUN' | 'DONE'

interface Waypoint {
  n: number
  e: number
  d: number
  hover_s: number
}

/** 24 m square patrol at 8 m AGL, takeoff + landing at home. */
function sortieRoute(): Waypoint[] {
  const A = 18
  return [
    { n: 0, e: 0, d: -8, hover_s: 0 },
    { n: A, e: A, d: -8, hover_s: 1.5 },
    { n: -A, e: A, d: -8, hover_s: 1.5 },
    { n: -A, e: -A, d: -8, hover_s: 1.5 },
    { n: A, e: -A, d: -8, hover_s: 1.5 },
    { n: 0, e: 0, d: -8, hover_s: 0 },
    { n: 0, e: 0, d: 0, hover_s: 0 },
  ]
}

export class SimMockEngine {
  private rng = mulberry32(0x51757)
  private tUs = 0
  private phase: Phase = 'WAIT'
  private phaseUntil = 2.2 // WAIT: PX4 boot handshake
  private sortie = 1

  private pos: [number, number, number] = [0, 0, 0]
  private vel: [number, number, number] = [0, 0, 0]
  private yaw = 0
  private roll = 0 // rad
  private pitch = 0 // rad
  private motors = [0, 0, 0, 0]
  private battery = 100

  private route: Waypoint[] = sortieRoute()
  private wp = 0
  private hoverLeft = 0

  private faults = new Map<string, MockFault>()
  private faultSeq = 1
  private baroDrift = 0

  private sentCount = 0
  private recvCount = 0
  private p95 = 124

  /** Advance the sim by dt seconds and build the frame. */
  tick(dt: number): SimFrame {
    this.tUs += Math.round(dt * 1e6)
    this.updatePhase(dt)
    if (this.phase === 'RUN') this.navigate(dt)
    else this.spoolDown(dt)
    this.applyFaults(dt)
    let dy = this.yaw - this.prevYaw
    while (dy > Math.PI) dy -= 2 * Math.PI
    while (dy < -Math.PI) dy += 2 * Math.PI
    this.yawRate = dy / Math.max(dt, 1e-3)
    this.prevYaw = this.yaw
    return this.frame()
  }

  private updatePhase(dt: number) {
    this.phaseUntil -= dt
    if (this.phaseUntil > 0) return
    if (this.phase === 'WAIT') {
      this.phase = 'RUN'
      this.route = sortieRoute()
      this.wp = 0
      this.hoverLeft = 0
      this.phaseUntil = Infinity
    } else if (this.phase === 'DONE') {
      // run boundary → new sortie (fresh battery if depleted)
      if (this.battery < 25) this.battery = 100
      this.sortie++
      this.tUs = 0
      this.phase = 'WAIT'
      this.phaseUntil = 2.0
      this.pos = [0, 0, 0]
      this.vel = [0, 0, 0]
    }
  }

  private navigate(dt: number) {
    const wp = this.route[this.wp] ?? this.route[this.route.length - 1]
    const dn = wp.n - this.pos[0]
    const de = wp.e - this.pos[1]
    const dd = wp.d - this.pos[2]
    const dist = Math.hypot(dn, de)

    if (this.hoverLeft > 0) {
      this.hoverLeft -= dt
      this.attitudeToward(dt, 0, 0, null)
      this.vel = [0, 0, 0]
    } else if (dist < 0.5 && Math.abs(dd) < 0.35) {
      this.wp++
      this.hoverLeft = wp.hover_s
      if (this.wp >= this.route.length) {
        this.phase = 'DONE'
        this.phaseUntil = 6.5
        return
      }
    } else {
      // horizontal velocity command: full 4 m/s cruise, brake near the target
      const vMax = Math.min(4, Math.max(0.4, dist * 0.9))
      const dirN = dist > 1e-6 ? dn / dist : 0
      const dirE = dist > 1e-6 ? de / dist : 0
      const desVn = dirN * vMax
      const desVe = dirE * vMax
      const aMax = 3.0
      const accN = clamp((desVn - this.vel[0]) / Math.max(dt, 1e-3), -aMax, aMax)
      const accE = clamp((desVe - this.vel[1]) / Math.max(dt, 1e-3), -aMax, aMax)
      // vertical: gentle 1.8 m/s
      const desVd = clamp(dd * 0.7, -1.8, 1.8)
      const accD = clamp((desVd - this.vel[2]) / Math.max(dt, 1e-3), -2.0, 2.0)

      this.vel[0] += accN * dt
      this.vel[1] += accE * dt
      this.vel[2] += accD * dt

      const yawCmd = Math.hypot(this.vel[0], this.vel[1]) > 0.4 ? Math.atan2(this.vel[1], this.vel[0]) : this.yaw
      this.attitudeToward(dt, accE, accN, yawCmd)
    }

    this.pos[0] += this.vel[0] * dt
    this.pos[1] += this.vel[1] * dt
    this.pos[2] += this.vel[2] * dt

    // battery drain ~ mean motor output
    const meanMotor = this.motors.reduce((a, b) => a + b, 0) / 4
    if (meanMotor > 0.08) {
      this.battery = Math.max(0, this.battery - dt * 0.42 * (0.4 + meanMotor))
      if (this.battery < 12) {
        // low-battery abort → land where we are (safety ladder flavour)
        this.route = [
          { n: this.pos[0], e: this.pos[1], d: this.pos[2], hover_s: 0 },
          { n: this.pos[0], e: this.pos[1], d: 0, hover_s: 0 },
        ]
        this.wp = 0
        this.hoverLeft = 0
      }
    }
    this.updateMotors(dt)
  }

  private spoolDown(_dt: number) {
    this.motors = this.motors.map((m) => m * 0.9)
    this.attitudeToward(_dt, 0, 0, null)
    this.vel = [0, 0, 0]
    if (this.phase === 'DONE') this.pos[2] = Math.min(0, this.pos[2] * 0.9 + 0.02)
  }

  /** Track a commanded tilt (from NED accel) + optional yaw heading. */
  private attitudeToward(dt: number, accE: number, accN: number, yawCmd: number | null) {
    const rollCmd = Math.atan2(accE, G) // bank into east accel
    const pitchCmd = Math.atan2(accN, G) // nose toward north accel
    const k = Math.min(1, dt * 2.6)
    this.roll += (rollCmd - this.roll) * k
    this.pitch += (pitchCmd - this.pitch) * k
    if (yawCmd != null) {
      let dy = yawCmd - this.yaw
      while (dy > Math.PI) dy -= 2 * Math.PI
      while (dy < -Math.PI) dy += 2 * Math.PI
      this.yaw += dy * Math.min(1, dt * 2.2)
    }
  }

  private updateMotors(dt: number) {
    const meanMotor = this.motors.reduce((a, b) => a + b, 0) / 4
    const climb = this.vel[2] < -0.05 ? 1 : this.vel[2] > 0.05 ? -0.6 : 0
    const flying = this.phase === 'RUN'
    let base = flying ? clamp(0.44 + climb * 0.1 + Math.hypot(this.roll, this.pitch) * 0.25, 0.05, 0.95) : meanMotor * 0.9
    base = clamp(base + gaussian(this.rng, 0.008), 0, 1)
    const r = this.roll
    const p = this.pitch
    const mix = [
      base + r * 0.18 - p * 0.12,
      base - r * 0.18 - p * 0.12,
      base - r * 0.18 + p * 0.12,
      base + r * 0.18 + p * 0.12,
    ].map((m) => clamp(m + gaussian(this.rng, 0.006), 0, 1))
    this.motors = mix
  }

  private applyFaults(dt: number) {
    for (const [id, f] of this.faults) {
      if (f.af.ttl_s != null) {
        f.af.ttl_s -= dt
        if (f.af.ttl_s <= 0) {
          this.faults.delete(id)
          continue
        }
      }
      const e = f.effect
      switch (e.kind) {
        case 'gps_denial':
          break // handled in frame()
        case 'motor_eff':
          if (e.motor != null && e.factor != null) this.motors[e.motor] *= e.factor
          break
        case 'motor_cut':
          if (e.motor != null) {
            this.motors[e.motor] = 0
            // asymmetric loss: yaw drift + sag
            this.yaw += dt * 0.35
            this.pitch += dt * 0.02
            this.pos[2] += dt * 0.25
          }
          break
        case 'baro_drift':
          if (e.rate_m_s != null) this.baroDrift += e.rate_m_s * dt
          break
        case 'wind':
          if (e.riseT == null || e.riseT <= 0) break
          // gust pushes the airframe around while the controller fights it
          this.pos[0] += (e.windN ?? 0) * dt * 0.12
          this.pos[1] += (e.windE ?? 0) * dt * 0.12
          this.roll += gaussian(this.rng, 0.02 * Math.abs(e.windN ?? 0) + 0.005)
          this.pitch += gaussian(this.rng, 0.02 * Math.abs(e.windE ?? 0) + 0.005)
          break
        default:
          break
      }
    }
    this.motors = this.motors.map((m) => clamp(m, 0, 1))
  }

  private frame(): SimFrame {
    const rng = this.rng
    const wind = this.activeEffect('wind')
    const gpsDenied = this.hasKind('gps_denial')

    // --- sensors (last emitted noisy values, per SPEC §4.2) ---
    let ax = G * Math.sin(this.pitch) + gaussian(rng, 0.04)
    let ay = -G * Math.sin(this.roll) + gaussian(rng, 0.04)
    let az = -G * Math.cos(this.pitch) * Math.cos(this.roll) + gaussian(rng, 0.05)
    let gx = gaussian(rng, 0.0016)
    let gy = gaussian(rng, 0.0016)
    let gz = gaussian(rng, 0.0016)
    for (const f of this.faultsList()) {
      const e = f.effect
      if (e.kind === 'imu_bias' && e.axis != null) {
        const age = (Date.now() - f.af.since) / 1000
        const ramp = (e.rate ?? 0) * age
        const arr = e.sensor === 'gyro' ? ((): [number, number, number] => [gx, gy, gz])() : ((): [number, number, number] => [ax, ay, az])()
        arr[e.axis] += e.sensor === 'gyro' ? ramp * 0.05 : ramp * G * 0.5
        if (e.sensor === 'gyro') [gx, gy, gz] = arr
        else [ax, ay, az] = arr
      }
      if (e.kind === 'imu_sat' && e.axis != null && e.limit != null) {
        const arr = e.sensor === 'gyro' ? [gx, gy, gz] : [ax, ay, az]
        arr[e.axis] = clamp(arr[e.axis], -e.limit, e.limit)
        if (e.sensor === 'gyro') [gx, gy, gz] = arr as [number, number, number]
        else [ax, ay, az] = arr as [number, number, number]
      }
    }

    const baro = -this.pos[2] + gaussian(rng, 0.12) + this.baroDrift + (wind ? gaussian(rng, 0.05) : 0)

    const gpsSat = gpsDenied ? 0 : Math.round(10 + gaussian(rng, 1.2))
    const gpsFix = gpsDenied ? 0 : gpsSat > 8 ? 3 : gpsSat > 5 ? 2 : 1

    // --- truth attitude as quaternion (ZYX, NED) ---
    const q = eulerToQuat(this.roll, this.pitch, this.yaw)

    // --- counters: lockstep ticks at ~250 Hz ---
    const dtSent = Math.round(2.5) // per 10 Hz frame → 250 Hz
    this.sentCount += dtSent
    this.recvCount += Math.max(0, dtSent - (Math.random() < 0.08 ? 1 : 0))
    this.p95 = clamp(this.p95 + gaussian(rng, 6), 95, 260)
    if (this.hasKind('delay')) this.p95 = clamp(this.p95 + gaussian(rng, 10) + 20, 95, 400)

    return {
      t_us: this.tUs,
      phase: this.phase,
      state: {
        pos_ned_m: [...this.pos],
        vel_ned_ms: [...this.vel],
        q_wxyz: q,
        omega_body_rads: [
          gaussian(rng, 0.002) + (this.hasKind('motor_cut') ? 0.18 : 0),
          gaussian(rng, 0.002),
          this.yawRate + gaussian(rng, 0.002),
        ],
        motors: [...this.motors],
        battery_pct: this.battery,
      },
      sensors: {
        accel_ms2: [ax, ay, az],
        gyro_rads: [gx, gy, gz],
        baro_alt_m: baro,
        gps_fix: gpsFix,
        gps_sat: Math.max(0, gpsSat),
      },
      faults_active: this.faultsList().map((f) => f.af),
      stats: {
        tick_p95_us: Math.round(this.p95),
        sent: { hil_sensor: this.sentCount, hil_gps: Math.round(this.sentCount * 0.5) },
        recv: { hil_actuator_controls: this.recvCount },
      },
    }
  }

  private yawRate = 0
  private prevYaw = 0
  private hasKind(kind: MockFault['effect']['kind']): boolean {
    return this.faultsList().some((f) => f.effect.kind === kind)
  }
  private activeEffect(kind: MockFault['effect']['kind']): MockFault | undefined {
    return this.faultsList().find((f) => f.effect.kind === kind)
  }
  private faultsList(): MockFault[] {
    return [...this.faults.values()]
  }

  get sortieNo(): number {
    return this.sortie
  }

  /** Supervisor-visible status (REST /api/status shape). */
  status(): SimStatus {
    const booted = !(this.phase === 'WAIT' && this.phaseUntil > 1.2)
    return {
      phase: this.phase,
      px4_connected: booted,
      loop_closed: booted && this.phase === 'RUN',
      tick_p95_us: Math.round(this.p95),
      rate_hz: booted ? 10.0 : 0,
      t_us: this.tUs,
    }
  }

  /** POST /api/faults — same event schema, start_ms = now. */
  injectFault(params: Record<string, number | string>): ActiveFault {
    const type = String(params.type ?? '')
    const id = `f${this.faultSeq++}`
    const numP = (k: string, d: number) => (typeof params[k] === 'number' ? (params[k] as number) : Number(params[k] ?? d) || d)
    const strP = (k: string, d: string) => String(params[k] ?? d)
    const axis = (a: string): 0 | 1 | 2 => (a === 'y' ? 1 : a === 'z' ? 2 : 0)

    let effect: MockFault['effect']
    let ttl_s: number | null = null
    const echo: Record<string, number | string> = {}
    switch (type) {
      case 'motor_efficiency':
        effect = { kind: 'motor_eff', motor: numP('motor', 0), factor: numP('factor', 0.55) }
        echo.motor = effect.motor!
        echo.factor = effect.factor!
        ttl_s = 45
        break
      case 'motor_cut':
        effect = { kind: 'motor_cut', motor: numP('motor', 0) }
        echo.motor = effect.motor!
        ttl_s = 30
        break
      case 'imu_bias':
        effect = { kind: 'imu_bias', sensor: strP('sensor', 'gyro') as 'gyro' | 'accel', axis: axis(strP('axis', 'x')), rate: numP('rate', 0.02) }
        echo.sensor = effect.sensor ?? 'gyro'
        echo.axis = strP('axis', 'x')
        echo.rate = effect.rate!
        ttl_s = null
        break
      case 'imu_saturation':
        effect = { kind: 'imu_sat', sensor: strP('sensor', 'accel') as 'gyro' | 'accel', axis: axis(strP('axis', 'z')), limit: numP('limit', 0.5) }
        echo.sensor = effect.sensor ?? 'accel'
        echo.axis = strP('axis', 'z')
        echo.limit = effect.limit!
        ttl_s = null
        break
      case 'gps_denial':
        effect = { kind: 'gps_denial' }
        ttl_s = numP('duration_s', 20)
        echo.duration_s = ttl_s
        break
      case 'gps_glitch':
        effect = { kind: 'gps_glitch' }
        ttl_s = numP('duration_s', 15)
        echo.offset_m = numP('offset_m', 8)
        echo.duration_s = ttl_s
        break
      case 'baro_drift':
        effect = { kind: 'baro_drift', rate_m_s: numP('rate_m_s', 0.1) }
        echo.rate_m_s = effect.rate_m_s!
        ttl_s = null
        break
      case 'wind_event':
        effect = { kind: 'wind', windN: numP('wind_n', 5), windE: numP('wind_e', -3), riseT: numP('rise_s', 3) }
        echo.wind_n = effect.windN!
        echo.wind_e = effect.windE!
        echo.rise_s = effect.riseT!
        ttl_s = 40
        break
      case 'transport_delay':
        effect = { kind: 'delay' }
        echo.ms = numP('ms', 40)
        ttl_s = null
        break
      case 'packet_drop':
        effect = { kind: 'drop' }
        echo.percent = numP('percent', 15)
        ttl_s = null
        break
      default:
        effect = { kind: 'gps_denial' }
        ttl_s = 10
    }
    if (params.duration_s != null && ttl_s == null) ttl_s = Number(params.duration_s)

    const af: ActiveFault = { id, type, params: echo, since: Date.now(), ttl_s }
    this.faults.set(id, { af, effect })
    return af
  }

  /** DELETE /api/faults/{id} */
  clearFault(id: string): boolean {
    if (!this.faults.has(id)) return false
    this.faults.delete(id)
    return true
  }

  /** POST /api/estop — stop the run at end of current tick. */
  estop(): void {
    this.phase = 'DONE'
    this.phaseUntil = 10
    this.motors = [0, 0, 0, 0]
    this.vel = [0, 0, 0]
    this.faults.clear()
  }
}

function eulerToQuat(roll: number, pitch: number, yaw: number): [number, number, number, number] {
  const cr = Math.cos(roll / 2)
  const sr = Math.sin(roll / 2)
  const cp = Math.cos(pitch / 2)
  const sp = Math.sin(pitch / 2)
  const cy = Math.cos(yaw / 2)
  const sy = Math.sin(yaw / 2)
  return [
    cr * cp * cy + sr * sp * sy,
    sr * cp * cy - cr * sp * sy,
    cr * sp * cy + sr * cp * sy,
    cr * cp * sy - sr * sp * cy,
  ]
}
