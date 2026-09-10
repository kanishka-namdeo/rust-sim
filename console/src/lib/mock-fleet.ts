/**
 * Client-side stand-in for the mavfleet fleet plane (SPEC §3.4, §4–§8).
 *
 * Used when the Rust backend on :8400 is unreachable. Produces fleet frames
 * in the live 10 Hz schema (the mock engine itself ticks at 5 Hz — cadence
 * differs, shape does not).
 * /ws/fleet summary shape as GET /api/fleet: three vehicles with FSM churn,
 * contract-net task allocation, the §8.1 safety ladder, geofence, and a
 * streaming event log — so the Fleet C2 UI is fully exercisable offline.
 */

import { clamp, gaussian, mulberry32 } from './format'
import { DEFAULT_ORIGIN, geodeticToNed, nedToGeodetic, type GeoOrigin } from './geo'
import type {
  FleetEvent,
  FleetSnapshot,
  FleetTask,
  FleetVehicle,
  FsmState,
  Geofence,
  MapWaypoint,
  MissionUploadResult,
} from './types'

const CRUISE_MS = 4.0
const FENCE: Geofence = {
  points: [
    [-45, -45],
    [45, -45],
    [45, 45],
    [-45, 45],
  ],
  ceiling_m: 60,
  floor_m: 0,
}

interface Mv {
  id: string
  index: number
  sysid: number
  home: [number, number]
  pos: [number, number, number]
  vel: [number, number, number]
  yaw: number
  fsm: FsmState
  mode: string
  armed: boolean
  battery: number
  hbAge: number
  hbLoss: number // virtual seconds remaining of induced heartbeat loss
  health: Set<string>
  queue: string[]
  hoverLeft: number
  stateTimer: number
  breadcrumb: { n: number; e: number }[]
  crumbTimer: number
  warnedFence: boolean
  batteryLowActed: boolean
  /** ADR-0017: operator go-to target (mock flies it in OFFBOARD). */
  gotoTarget: [number, number, number] | null
}

export interface FleetTick {
  snapshot: FleetSnapshot
  events: FleetEvent[]
}

const NAMES = ['ALPHA', 'BRAVO', 'CHARLIE']
const HOMES: [number, number][] = [
  [-20, -38],
  [0, -38],
  [20, -38],
]

export class FleetMockEngine {
  private rng = mulberry32(0xfe17)
  private tS = 0
  private phase: 'INIT' | 'RUNNING' | 'ABORTED' | 'SETUP_HOLD' = 'INIT'
  private vehicles: Mv[]
  private tasks = new Map<string, FleetTask>()
  private events: FleetEvent[] = []
  private round = 0
  private taskSeq = 1
  private opSeq = 0
  private hbNext = 34
  private appendNext = 22
  /** ADR-0017: the geo origin the mock NED frame anchors to. */
  readonly geoOrigin: GeoOrigin = DEFAULT_ORIGIN

  constructor() {
    this.vehicles = NAMES.map((id, i) => {
      const v: Mv = {
        id,
        index: i,
        sysid: i + 1,
        home: HOMES[i],
        pos: [HOMES[i][0], HOMES[i][1], 0],
        vel: [0, 0, 0],
        yaw: 0,
        fsm: 'INIT',
        mode: '—',
        armed: false,
        battery: [96, 88, 71][i],
        hbAge: 0.2,
        hbLoss: 0,
        health: new Set<string>(),
        queue: [],
        hoverLeft: 0,
        stateTimer: 1.2 + i * 0.8,
        breadcrumb: [],
        crumbTimer: 0,
        warnedFence: false,
        batteryLowActed: false,
        gotoTarget: null,
      }
      return v
    })
    for (let i = 0; i < 8; i++) this.addTask(false)
    this.emit('boundary', null, 'run start: scenario "patrol-sq3" — 3 vehicles, 8 tasks, fence ±45 m', 'info')
  }

  /** Advance the fleet by dt seconds (call at 5 Hz). */
  tick(dt: number): FleetTick {
    const evBefore = this.events.length
    if (this.phase === 'INIT') {
      this.tS += dt
      if (this.tS > 2.5) {
        this.phase = 'RUNNING'
        this.emit('boundary', null, 'all vehicles spawned — fleet RUNNING', 'info')
      }
    } else {
      this.tS += dt
    }

    for (const v of this.vehicles) this.stepVehicle(v, dt)

    if (this.phase === 'RUNNING') {
      this.allocate()
      this.scheduleChurn(dt)
    }

    return {
      snapshot: this.snapshot(),
      events: this.events.slice(evBefore),
    }
  }

  // ---------------------------------------------------------------- vehicle

  private stepVehicle(v: Mv, dt: number) {
    // heartbeat / staleness
    if (v.hbLoss > 0) {
      v.hbLoss -= dt
      v.hbAge += dt
      if (v.hbAge > 1.5) v.health.add('LINK_STALE')
      if (v.hbAge > 3.0) v.health.add('HEARTBEAT_LOST')
    } else {
      v.hbAge = Math.max(0, 0.15 + gaussian(this.rng, 0.08))
      v.health.delete('LINK_STALE')
      v.health.delete('HEARTBEAT_LOST')
    }

    // ADR-0017: operator go-to flight (OFFBOARD to the clicked point)
    if (v.gotoTarget) {
      v.mode = 'Offboard'
      v.armed = true
      const prog = this.navigate(v, v.gotoTarget, dt)
      if (prog > 0.999 && v.pos[2] > v.gotoTarget[2] - 0.5) {
        // arrived: hold at the target
        v.gotoTarget = null
        this.emit('supervisor', v.id, 'go-to target reached — holding (OFFBOARD stream live)', 'info')
      }
      return
    }

    switch (v.fsm) {
      case 'INIT':
        v.stateTimer -= dt
        if (v.stateTimer <= 0) this.fsm(v, 'SPAWNING', 'supervisor started bring-up')
        break
      case 'SPAWNING':
        v.stateTimer -= dt
        if (v.stateTimer <= 0) this.fsm(v, 'BOOTING', 'px4 launched, sim TCP accepted')
        break
      case 'BOOTING':
        v.stateTimer -= dt
        if (v.stateTimer <= 0) this.fsm(v, 'READY', 'heartbeats + local position valid + home set')
        break
      case 'READY':
        v.mode = 'Posctl'
        if (v.battery < 14) {
          this.emit('supervisor', v.id, 'battery swap complete (pack replaced) — 100 %', 'info')
          v.battery = 100
          v.batteryLowActed = false
        }
        break
      case 'ACTIVE': {
        v.mode = 'Offboard'
        v.armed = v.pos[2] < -0.5 || v.queue.length > 0
        const taskId = v.queue[0]
        const task = taskId ? this.tasks.get(taskId) : undefined
        if (!task) {
          v.queue = []
          this.fsm(v, 'READY', 'queue empty')
          break
        }
        if (task.status === 'assigned') {
          task.status = 'in_progress'
          this.emit('fsm', v.id, `offboard engaged → transit ${task.id} (layer ${10 + 5 * v.index} m)`, 'info')
        }
        task.progress = this.navigate(v, task.pos_ned_m, dt)
        if (task.progress > 0.999) {
          if (v.hoverLeft === 0) v.hoverLeft = task.hover_s
          v.hoverLeft -= dt
          if (v.hoverLeft <= 0) {
            task.status = 'done'
            task.progress = 1
            v.queue.shift()
            this.emit('boundary', v.id, `task ${task.id} complete (hover ${task.hover_s.toFixed(0)} s)`, 'info')
            if (v.queue.length === 0) this.fsm(v, 'READY', 'task complete')
          }
        }
        // battery ladder while active
        if (v.battery < 20 && !v.batteryLowActed) {
          this.emit('supervisor', v.id, 'policy 4 (battery critical <20 %): LAND', 'critical')
          this.fsm(v, 'LANDED', 'battery critical', 'supervisor policy 4')
          v.batteryLowActed = true
        } else if (v.battery < 30 && v.queue.length > 1 && !v.batteryLowActed) {
          this.emit('supervisor', v.id, 'policy 5 (battery low <30 %): RTL after current task, no new tasks', 'warn')
          v.queue = v.queue.slice(0, 1)
          v.batteryLowActed = true
        }
        break
      }
      case 'RTL': {
        v.mode = 'Auto RTL'
        const done = this.navigate(v, [v.home[0], v.home[1], 0], dt)
        if (done > 0.999) {
          v.armed = false
          this.fsm(v, 'LANDED', 'disarm observed')
        }
        break
      }
      case 'LANDED':
        v.mode = 'Disarmed'
        v.armed = false
        v.gotoTarget = null
        v.pos = [v.pos[0], v.pos[1], 0]
        v.vel = [0, 0, 0]
        v.stateTimer -= dt
        if (v.stateTimer <= 0 && this.phase === 'RUNNING') this.fsm(v, 'READY', 're-arm cycle allowed by scenario')
        break
      case 'FAULT':
        v.mode = '—'
        break
    }

    // geofence proximity warning (within 10 m of boundary)
    const nearEdge =
      Math.abs(v.pos[0]) > FENCE.points[1][0] - 10 ||
      Math.abs(v.pos[1]) > FENCE.points[2][1] - 10
    if (nearEdge && !v.warnedFence && v.fsm === 'ACTIVE') {
      v.health.add('GEOFENCE_WARN')
      v.warnedFence = true
      this.emit('supervisor', v.id, 'policy 2 pre-arm: within 10 m of geofence boundary', 'warn')
    } else if (!nearEdge && v.warnedFence) {
      v.health.delete('GEOFENCE_WARN')
      v.warnedFence = false
    }

    // battery drain while airborne
    if (v.pos[2] < -0.5) v.battery = Math.max(0, v.battery - dt * 0.16)
    if (v.battery < 30) v.health.add('BATTERY_LOW')
    if (v.battery < 20) v.health.add('BATTERY_CRIT')
    if (v.battery >= 30) {
      v.health.delete('BATTERY_LOW')
      v.health.delete('BATTERY_CRIT')
    }

    // breadcrumbs (~0.4 s cadence, 2 s trail)
    v.crumbTimer -= dt
    if (v.crumbTimer <= 0) {
      v.crumbTimer = 0.4
      v.breadcrumb.push({ n: v.pos[0], e: v.pos[1] })
      if (v.breadcrumb.length > 6) v.breadcrumb.shift()
    }
  }

  /** Move toward a NED target; returns progress 0..1. */
  private navigate(v: Mv, target: [number, number, number], dt: number): number {
    const startDist = Math.max(1e-6, Math.hypot(target[0] - v.pos[0], target[1] - v.pos[1]))
    const dn = target[0] - v.pos[0]
    const de = target[1] - v.pos[1]
    const dd = target[2] - v.pos[2]
    const distH = Math.hypot(dn, de)
    const layerClear = distH < 15
    const cruiseD = layerClear ? target[2] : -(10 + 5 * v.index)

    const vMax = Math.min(CRUISE_MS, Math.max(0.3, distH * 0.8))
    const dirN = distH > 1e-6 ? dn / distH : 0
    const dirE = distH > 1e-6 ? de / distH : 0
    const desVn = dirN * vMax
    const desVe = dirE * vMax
    const desVd = clamp((cruiseD - v.pos[2]) * 0.7, -2, 2)
    const aMax = 2.5
    v.vel[0] += clamp((desVn - v.vel[0]) / Math.max(dt, 1e-3), -aMax, aMax) * dt
    v.vel[1] += clamp((desVe - v.vel[1]) / Math.max(dt, 1e-3), -aMax, aMax) * dt
    v.vel[2] += clamp((desVd - v.vel[2]) / Math.max(dt, 1e-3), -2, 2) * dt

    v.pos[0] += v.vel[0] * dt
    v.pos[1] += v.vel[1] * dt
    v.pos[2] = clamp(v.pos[2] + v.vel[2] * dt, -FENCE.ceiling_m, 0)

    if (Math.hypot(v.vel[0], v.vel[1]) > 0.4) v.yaw = Math.atan2(v.vel[1], v.vel[0])

    const nowDist = Math.hypot(target[0] - v.pos[0], target[1] - v.pos[1])
    const horiz = 1 - clamp(nowDist / startDist, 0, 1)
    const vert = 1 - clamp(Math.abs(dd) / 12, 0, 1)
    return Math.min(horiz, distH < 0.5 ? vert : horiz)
  }

  // ------------------------------------------------------------- allocator

  private allocate() {
    const pending = [...this.tasks.values()]
      .filter((t) => t.status === 'pending')
      .sort((a, b) => b.reward - a.reward)
    const eligible = (v: Mv) =>
      (v.fsm === 'READY' || (v.fsm === 'ACTIVE' && v.queue.length < 2)) &&
      !v.health.has('BATTERY_LOW')
    for (const task of pending) {
      const bidders = this.vehicles.filter(eligible)
      if (bidders.length === 0) return
      let best: Mv | null = null
      let bestBid = Infinity
      for (const v of bidders) {
        const bid = this.bid(v, task)
        if (bid < bestBid) {
          bestBid = bid
          best = v
        }
      }
      if (!best) continue
      this.round++
      const round = this.round
      const bidS = Math.round(bestBid * 10) / 10
      const biddersN = bidders.length
      task.status = 'assigned'
      task.assigned_to = best.id
      best.queue.push(task.id)
      this.emit(
        'award',
        best.id,
        `round ${round}: task ${task.id} → ${best.id} (bid ${bidS.toFixed(1)} s, ${biddersN} bidders)`,
        'info',
      )
      if (best.fsm === 'READY') this.fsm(best, 'ACTIVE', 'task assigned and accepted')
    }
  }

  /** SPEC §6.2 bid: travel + queue + soft battery wall. */
  private bid(v: Mv, t: FleetTask): number {
    const dist = Math.hypot(t.pos_ned_m[0] - v.pos[0], t.pos_ned_m[1] - v.pos[1])
    const travel = (dist * 1.25) / CRUISE_MS
    const queue = v.queue.length * 14
    let penalty = 0
    if (v.battery < 45) {
      const x = clamp((45 - v.battery) / 20, 0, 1) // 1 at 25 %
      penalty = 600 * x * x
    }
    return travel + queue + penalty + t.hover_s * 0.5
  }

  // ---------------------------------------------------------------- churn

  private scheduleChurn(dt: number) {
    this.hbNext -= dt
    if (this.hbNext <= 0) {
      this.hbNext = 30 + this.rng() * 25
      const actives = this.vehicles.filter((v) => v.fsm === 'ACTIVE')
      if (actives.length > 0) {
        const v = actives[Math.floor(this.rng() * actives.length)]
        v.hbLoss = 2.6 + this.rng() * 1.8
        this.emit('link', v.id, 'link degradation: heartbeat reception intermittent', 'warn')
      }
    }
    // resolve heartbeat losses
    for (const v of this.vehicles) {
      if (v.hbLoss > 0 && v.hbLoss <= dt) {
        if (v.hbAge > 3 && v.fsm === 'ACTIVE') {
          this.emit('supervisor', v.id, 'policy 3 (heartbeat loss >3 s): RTL — verifying mode change, escalate at 10 s', 'critical')
          this.fsm(v, 'RTL', 'heartbeat loss', 'supervisor policy 3')
        } else if (v.hbAge > 1.5) {
          this.emit('link', v.id, `link restored after ${v.hbAge.toFixed(1)} s (stale window)`, 'info')
        }
      }
    }

    this.appendNext -= dt
    if (this.appendNext <= 0) {
      this.appendNext = 18 + this.rng() * 18
      const t = this.addTask(true)
      if (t) this.emit('boundary', null, `task ${t.id} appended at runtime — triggers reallocation`, 'info')
    }
  }

  private addTask(_announce: boolean): FleetTask {
    const id = `T${this.taskSeq++}`
    const pos: [number, number, number] = [
      Math.round((this.rng() * 2 - 1) * 34),
      Math.round((this.rng() * 2 - 1) * 34),
      -(12 + Math.round(this.rng() * 14)),
    ]
    const task: FleetTask = {
      id,
      pos_ned_m: pos,
      hover_s: 3 + Math.round(this.rng() * 4),
      reward: 10 + Math.round(this.rng() * 40),
      status: 'pending',
      assigned_to: null,
      progress: 0,
    }
    this.tasks.set(id, task)
    return task
  }

  // ----------------------------------------------------------------- utils

  private fsm(v: Mv, to: FsmState, cause: string, actor = 'internal') {
    const from = v.fsm
    v.fsm = to
    v.stateTimer = to === 'SPAWNING' ? 1.5 : to === 'BOOTING' ? 2.2 : to === 'LANDED' ? 2.5 : 0
    v.hoverLeft = 0
    if (to === 'RTL' || to === 'LANDED') v.queue = []
    this.emit('fsm', v.id, `${from} → ${to} (${cause}) [${actor}]`, to === 'FAULT' ? 'critical' : 'info')
  }

  private emit(kind: string, vehicle: string | null, detail: string, severity: FleetEvent['severity']) {
    this.events.push({ t: Date.now(), t_s: Math.round(this.tS * 10) / 10, kind, vehicle, detail, severity })
    if (this.events.length > 400) this.events.shift()
  }

  private snapshot(): FleetSnapshot {
    const vehicles: FleetVehicle[] = this.vehicles.map((v) => {
      // ADR-0017: derive the geo fix from the mock NED state via the
      // geodesy port — exactly what the backend's HIL_GPS ->
      // GLOBAL_POSITION_INT chain produces.
      const g = nedToGeodetic(this.geoOrigin, v.pos)
      return {
        id: v.id,
        index: v.index,
        sysid: v.sysid,
        mode: v.mode,
        fsm: v.fsm,
        armed: v.armed,
        battery_pct: Math.round(v.battery * 10) / 10,
        voltage_v: 21.5 + (v.battery / 100) * 2.7,
        position_ned_m: [rnd2(v.pos[0]), rnd2(v.pos[1]), rnd2(v.pos[2])],
        velocity_ned_ms: [rnd2(v.vel[0]), rnd2(v.vel[1]), rnd2(v.vel[2])],
        lat: rnd6(g.lat),
        lon: rnd6(g.lng),
        alt_msl_m: rnd2(this.geoOrigin.alt_m - v.pos[2]),
        alt_agl_m: rnd2(-v.pos[2]),
        yaw_deg: Math.round(((v.yaw * 180) / Math.PI + 360) % 360),
        heartbeat_age_s: Math.round(v.hbAge * 100) / 100,
        stale: v.hbAge > 1.5,
        health: [...v.health],
        task_id: v.queue[0] ?? null,
        breadcrumb: v.breadcrumb,
      }
    })
    return {
      phase: this.phase,
      t_s: Math.round(this.tS * 10) / 10,
      vehicles,
      tasks: [...this.tasks.values()].map((t) => ({ ...t })),
      geofence: FENCE,
      geo_origin: {
        lat_deg: this.geoOrigin.lat_deg,
        lon_deg: this.geoOrigin.lon_deg,
        alt_m: this.geoOrigin.alt_m,
      },
    }
  }

  get eventLog(): FleetEvent[] {
    return this.events
  }

  get auctionLog(): never[] {
    // The auction log was removed (the swarm/autonomy auction log was
    // overkill for a GCS). Allocation still happens internally; the round
    // counter surfaces only via the 'award' event in the event log.
    return []
  }

  /** POST /api/fleet/estop — LAND all vehicles immediately, run ABORTED. */
  estop(): void {
    this.phase = 'ABORTED'
    this.emit('supervisor', null, 'policy 1 (operator E-STOP): all vehicles LAND immediately — run ABORTED', 'critical')
    for (const v of this.vehicles) {
      if (v.fsm === 'LANDED' || v.fsm === 'FAULT') continue
      if (v.pos[2] < -0.5) {
        this.fsm(v, 'LANDED', 'estop descent at position', 'supervisor policy 1')
        // fast descent to ground
        v.pos = [v.pos[0], v.pos[1], 0]
      } else {
        this.fsm(v, 'LANDED', 'on ground', 'supervisor policy 1')
      }
      v.hbLoss = 0
      v.gotoTarget = null
    }
  }

  // ------------------------------------------------------- operator plane
  // (ADR-0017 mock: the same semantics the fleet manager implements —
  // fence-validated upload, auction pickup, go-to, guided commands)

  /** POST /api/mission — validate + inject operator waypoints. */
  uploadMission(items: MapWaypoint[], replace: boolean): MissionUploadResult {
    const accepted: string[] = []
    const rejected: { label: string; reason: string }[] = []
    if (replace) this.clearMission()
    for (const wp of items) {
      const ned = geodeticToNed(this.geoOrigin, wp.lat, wp.lng, this.geoOrigin.alt_m + wp.alt_m)
      const inside =
        Math.abs(ned[0]) <= FENCE.points[1][0] && Math.abs(ned[1]) <= FENCE.points[2][1]
      const altOk = wp.alt_m >= FENCE.floor_m && wp.alt_m <= FENCE.ceiling_m
      if (!inside) {
        rejected.push({
          label: wp.key,
          reason: `outside geofence polygon (lat ${wp.lat.toFixed(6)}, lon ${wp.lng.toFixed(6)})`,
        })
        continue
      }
      if (!altOk) {
        rejected.push({
          label: wp.key,
          reason: `outside altitude box (AGL ${wp.alt_m.toFixed(1)} m; box ${FENCE.floor_m}..${FENCE.ceiling_m} m)`,
        })
        continue
      }
      const id = `op${++this.opSeq}`
      this.tasks.set(id, {
        id,
        pos_ned_m: [ned[0], ned[1], -wp.alt_m],
        hover_s: wp.hover_s,
        reward: 10,
        status: 'pending',
        assigned_to: null,
        progress: 0,
      })
      accepted.push(id)
    }
    if (accepted.length > 0) {
      this.emit(
        'supervisor',
        null,
        `operator mission upload: ${accepted.length} waypoint(s) accepted (${accepted.join(', ')}), ${rejected.length} rejected`,
        'info',
      )
    }
    const pending = [...this.tasks.values()].filter(
      (t) => t.status === 'pending' || t.status === 'assigned',
    ).length
    return { accepted, rejected, pool: pending }
  }

  /** POST /api/mission/start. */
  startMission(): { started: boolean; reason: string | null } {
    const pending = [...this.tasks.values()].filter((t) => t.status === 'pending')
    if (this.phase === 'ABORTED') return { started: false, reason: 'run aborted' }
    if (pending.length === 0) return { started: false, reason: 'no tasks to start — upload a mission first' }
    if (this.phase !== 'RUNNING') {
      this.phase = 'RUNNING'
      this.emit('boundary', null, 'operator mission start — the auction takes it from the next tick', 'info')
      return { started: true, reason: null }
    }
    return { started: false, reason: 'mission already started' }
  }

  /** POST /api/mission/clear. */
  clearMission(): number {
    let cleared = 0
    for (const t of this.tasks.values()) {
      if (t.id.startsWith('op') && (t.status === 'pending' || t.status === 'assigned')) {
        t.status = 'rejected'
        cleared++
      }
    }
    if (cleared > 0) this.emit('supervisor', null, `operator: cleared ${cleared} queued mission task(s)`, 'info')
    return cleared
  }

  private veh(index: number): Mv | null {
    return this.vehicles.find((v) => v.index === index) ?? null
  }

  /** POST /api/vehicles/{i}/arm. */
  arm(index: number, arm: boolean): { accepted: boolean; command: string } {
    const v = this.veh(index)
    if (!v) return { accepted: false, command: 'arm' }
    if (v.fsm === 'ACTIVE') {
      return { accepted: false, command: 'arm' }
    }
    v.armed = arm
    if (!arm) v.gotoTarget = null
    this.emit('supervisor', v.id, `operator: ${arm ? 'ARM' : 'DISARM'} via API — ACCEPTED`, 'info')
    return { accepted: true, command: arm ? 'arm' : 'disarm' }
  }

  /** POST /api/vehicles/{i}/takeoff. */
  takeoff(index: number, altM: number): { accepted: boolean; command: string } {
    const v = this.veh(index)
    if (!v) return { accepted: false, command: 'takeoff' }
    v.armed = true
    v.mode = 'Auto Takeoff'
    v.gotoTarget = [v.pos[0], v.pos[1], -altM]
    this.emit('supervisor', v.id, `operator: TAKEOFF to ${altM.toFixed(1)} m AGL via API — ACCEPTED`, 'info')
    return { accepted: true, command: 'takeoff' }
  }

  /** POST /api/vehicles/{i}/goto. */
  goto(index: number, lat: number, lng: number, altM: number): { accepted: boolean; command: string } {
    const v = this.veh(index)
    if (!v) return { accepted: false, command: 'goto' }
    const ned = geodeticToNed(this.geoOrigin, lat, lng, this.geoOrigin.alt_m + altM)
    // clamp into the fence (the backend's §7.2 rule)
    const x = clamp(ned[0], -FENCE.points[1][0], FENCE.points[1][0])
    const y = clamp(ned[1], -FENCE.points[2][1], FENCE.points[2][1])
    v.gotoTarget = [x, y, -altM]
    if (v.fsm === 'READY') this.fsm(v, 'ACTIVE', 'operator go-to accepted')
    this.emit(
      'supervisor',
      v.id,
      `operator: GO TO (${lat.toFixed(6)}, ${lng.toFixed(6)}) ${altM.toFixed(1)} m AGL — engage sequence started`,
      'info',
    )
    return { accepted: true, command: 'goto' }
  }

  /** POST /api/vehicles/{i}/{land,rtl,hold}. */
  land(index: number): { accepted: boolean; command: string } {
    const v = this.veh(index)
    if (!v) return { accepted: false, command: 'land' }
    v.gotoTarget = null
    v.mode = 'Auto Land'
    v.gotoTarget = [v.pos[0], v.pos[1], 0]
    this.emit('supervisor', v.id, 'operator: LAND via API — stream stopped, AUTO.LAND', 'info')
    return { accepted: true, command: 'land' }
  }

  rtl(index: number): { accepted: boolean; command: string } {
    const v = this.veh(index)
    if (!v) return { accepted: false, command: 'rtl' }
    v.gotoTarget = null
    if (v.fsm === 'ACTIVE') this.fsm(v, 'RTL', 'operator RTL')
    this.emit('supervisor', v.id, 'operator: RTL via API — stream stopped, AUTO.RTL', 'info')
    return { accepted: true, command: 'rtl' }
  }

  hold(index: number): { accepted: boolean; command: string } {
    const v = this.veh(index)
    if (!v) return { accepted: false, command: 'hold' }
    v.gotoTarget = [v.pos[0], v.pos[1], v.pos[2]]
    v.mode = 'Auto Loiter'
    this.emit('supervisor', v.id, 'operator: HOLD via API — stream stopped, AUTO.LOITER', 'info')
    return { accepted: true, command: 'hold' }
  }
}

function rnd2(x: number): number {
  return Math.round(x * 100) / 100
}

function rnd6(x: number): number {
  return Math.round(x * 1e6) / 1e6
}
