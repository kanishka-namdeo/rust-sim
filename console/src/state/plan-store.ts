/**
 * GCS v2 Operations Canvas — plan store (M10).
 *
 * Spec: docs/GCS_V2_SPEC.md §9.3 + §8.5 MissionStrip + §13.1 M10.
 *
 * `useSyncExternalStore` module store for the active mission plan:
 * the file (PlanMissionFile), dirty flag, validation result, upload
 * progress, download result, selected waypoint, undo stack.
 *
 * This is the v2 lift of v1 PlanView's local useState — the MissionStrip
 * overlay + the MapCanvas (L3/L4/L5/L7 layers) + the Library overlay all
 * read/write through this single store. The catalog REST calls
 * (save/validate/upload/download) go through the command bus; this store
 * holds the local state + the optimistic updates.
 *
 * The store is engine-neutral: the mock fallback (no catalog) keeps
 * editing local; the live catalog persists via POST/PUT.
 */

import { useSyncExternalStore } from 'react'
import type {
  PlanMissionFile,
  PlanValidationResult,
  PlanWaypoint,
  PlanGeofence,
  PlanRally,
} from '@/lib/plan-types'
import { emptyPlanMission, DEFAULT_WP_ALT_M, DEFAULT_WP_HOLD_S, DEFAULT_WP_ACCEPT_M } from '@/lib/plan-types'

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface UploadSubBar {
  status: 'idle' | 'pending' | 'success' | 'failed'
  itemsSent: number
  itemsAcked: number
  message: string
}

export interface UploadProgress {
  mission: UploadSubBar
  fence: UploadSubBar
  rally: UploadSubBar
}

export interface PlanStoreState {
  /** The active mission file (local draft). */
  file: PlanMissionFile
  /** Unsaved-changes flag (true after any edit, cleared on save). */
  dirty: boolean
  /** Last validation result (null = not validated). */
  validated: PlanValidationResult | null
  /** Upload progress per mission_type (0=mission, 1=fence, 2=rally). */
  uploadProgress: UploadProgress | null
  /** Download-from-vehicle result (for the comparison modal). */
  downloadResult: {
    ok: boolean
    status: number
    code: string | null
    error: string | null
    missionType: string
    items: PlanWaypoint[] | null
    rawItems: unknown[] | null
  } | null
  /** Selected waypoint seq (table + map highlight). */
  selectedWp: number | null
  /** Undo stack (§8.5 MissionStrip "undo stack") — last N file snapshots. */
  undoStack: PlanMissionFile[]
}

const INITIAL: PlanStoreState = {
  file: emptyPlanMission('untitled'),
  dirty: false,
  validated: null,
  uploadProgress: null,
  downloadResult: null,
  selectedWp: null,
  undoStack: [],
}

// ---------------------------------------------------------------------------
// Module-level store (useSyncExternalStore contract)
// ---------------------------------------------------------------------------

let state: PlanStoreState = INITIAL
const listeners = new Set<() => void>()
let version = 0

function setState(next: Partial<PlanStoreState>): void {
  state = { ...state, ...next }
  version++
  for (const fn of listeners) fn()
}

export function subscribe(fn: () => void): () => void {
  listeners.add(fn)
  return () => listeners.delete(fn)
}

export function getSnapshot(): PlanStoreState {
  return state
}

export function getVersion(): number {
  return version
}

export function usePlanStore(): PlanStoreState {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
}

// ---------------------------------------------------------------------------
// Waypoint mutations — the v1 PlanView addWaypoint/moveWaypoint/removeWaypoint/
// patchWaypoint ported to the store. Each pushes the prev file to the undo
// stack, sets dirty=true, clears validated + uploadProgress.
// ---------------------------------------------------------------------------

function pushUndo(): void {
  setState({ undoStack: [...state.undoStack.slice(-19), state.file] })
}

export function addWaypoint(lat: number, lng: number): void {
  pushUndo()
  const wps = state.file.waypoints
  const seq = wps.length > 0 ? Math.max(...wps.map((w) => w.seq)) + 1 : 0
  const wp: PlanWaypoint = {
    seq,
    frame: 3, // GLOBAL_RELATIVE_ALT
    command: 16, // NAV_WAYPOINT
    x: lat,
    y: lng,
    z: DEFAULT_WP_ALT_M,
    param1: DEFAULT_WP_HOLD_S,
    param2: DEFAULT_WP_ACCEPT_M,
    param3: 0,
    param4: 0,
  }
  setState({
    file: { ...state.file, waypoints: [...wps, wp] },
    dirty: true,
    validated: null,
    uploadProgress: null,
  })
}

export function moveWaypoint(seq: number, lat: number, lng: number): void {
  pushUndo()
  setState({
    file: {
      ...state.file,
      waypoints: state.file.waypoints.map((w) => (w.seq === seq ? { ...w, x: lat, y: lng } : w)),
    },
    dirty: true,
    validated: null,
    uploadProgress: null,
  })
}

export function removeWaypoint(seq: number): void {
  pushUndo()
  // Re-number seq to be contiguous (QGC behavior per v1 PlanView).
  const wps = state.file.waypoints
    .filter((w) => w.seq !== seq)
    .map((w, i) => ({ ...w, seq: i }))
  setState({
    file: { ...state.file, waypoints: wps },
    dirty: true,
    validated: null,
    uploadProgress: null,
    selectedWp: null,
  })
}

export function patchWaypoint(seq: number, partial: Partial<PlanWaypoint>): void {
  pushUndo()
  setState({
    file: {
      ...state.file,
      waypoints: state.file.waypoints.map((w) => (w.seq === seq ? { ...w, ...partial } : w)),
    },
    dirty: true,
    validated: null,
    uploadProgress: null,
  })
}

export function replaceWaypoints(wps: PlanWaypoint[]): void {
  pushUndo()
  setState({
    file: { ...state.file, waypoints: wps.map((w, i) => ({ ...w, seq: i })) },
    dirty: true,
    validated: null,
    uploadProgress: null,
    selectedWp: null,
  })
}

// ---------------------------------------------------------------------------
// Geofence mutations — addFenceVertex, closeFence, clearFence, addExclusionPolygon,
// deleteFenceVertex, insertFenceVertex (P7 fix).
// ---------------------------------------------------------------------------

export function addFenceVertex(lat: number, lng: number): void {
  pushUndo()
  const inclusion = [...state.file.geofence.inclusion, [lat, lng] as [number, number]]
  setState({
    file: { ...state.file, geofence: { ...state.file.geofence, inclusion } },
    dirty: true,
    validated: null,
    uploadProgress: null,
  })
}

export function closeFence(): boolean {
  // Returns false if < 3 vertices (the caller toasts destructive).
  if (state.file.geofence.inclusion.length < 3) return false
  return true
}

export function clearFence(): void {
  pushUndo()
  setState({
    file: {
      ...state.file,
      geofence: { ...state.file.geofence, inclusion: [], exclusion: [] },
    },
    dirty: true,
    validated: null,
    uploadProgress: null,
  })
}

export function addExclusionPolygon(): void {
  pushUndo()
  // Add an empty exclusion polygon (the operator clicks to add vertices).
  setState({
    file: {
      ...state.file,
      geofence: { ...state.file.geofence, exclusion: [...state.file.geofence.exclusion, []] },
    },
    dirty: true,
    validated: null,
    uploadProgress: null,
  })
}

export function addExclusionVertex(polyId: number, lat: number, lng: number): void {
  pushUndo()
  const exclusion = state.file.geofence.exclusion.map((poly, i) =>
    i === polyId ? [...poly, [lat, lng] as [number, number]] : poly,
  )
  setState({
    file: { ...state.file, geofence: { ...state.file.geofence, exclusion } },
    dirty: true,
    validated: null,
    uploadProgress: null,
  })
}

/** P7 fix: delete a single fence vertex. polyId 0 = inclusion, 1..N = exclusion. */
export function deleteFenceVertex(polyId: number, vtxId: number): void {
  pushUndo()
  if (polyId === 0) {
    const inclusion = state.file.geofence.inclusion.filter((_, i) => i !== vtxId)
    setState({
      file: { ...state.file, geofence: { ...state.file.geofence, inclusion } },
      dirty: true,
      validated: null,
      uploadProgress: null,
    })
  } else {
    const exclusion = state.file.geofence.exclusion.map((poly, i) =>
      i === polyId - 1 ? poly.filter((_, vi) => vi !== vtxId) : poly,
    )
    setState({
      file: { ...state.file, geofence: { ...state.file.geofence, exclusion } },
      dirty: true,
      validated: null,
      uploadProgress: null,
    })
  }
}

/** P7 fix: insert a vertex on a fence segment. */
export function insertFenceVertex(polyId: number, afterVtxId: number, lat: number, lng: number): void {
  pushUndo()
  if (polyId === 0) {
    const inclusion = [
      ...state.file.geofence.inclusion.slice(0, afterVtxId + 1),
      [lat, lng] as [number, number],
      ...state.file.geofence.inclusion.slice(afterVtxId + 1),
    ]
    setState({
      file: { ...state.file, geofence: { ...state.file.geofence, inclusion } },
      dirty: true,
      validated: null,
      uploadProgress: null,
    })
  } else {
    const exclusion = state.file.geofence.exclusion.map((poly, i) => {
      if (i !== polyId - 1) return poly
      return [
        ...poly.slice(0, afterVtxId + 1),
        [lat, lng] as [number, number],
        ...poly.slice(afterVtxId + 1),
      ]
    })
    setState({
      file: { ...state.file, geofence: { ...state.file.geofence, exclusion } },
      dirty: true,
      validated: null,
      uploadProgress: null,
    })
  }
}

export function setFenceCeilingFloor(ceiling_m: number, floor_m: number): void {
  pushUndo()
  setState({
    file: {
      ...state.file,
      geofence: { ...state.file.geofence, ceiling_m, floor_m },
    },
    dirty: true,
    validated: null,
    uploadProgress: null,
  })
}

// ---------------------------------------------------------------------------
// Rally mutations
// ---------------------------------------------------------------------------

export function addRallyPoint(lat: number, lng: number): void {
  pushUndo()
  const rally = state.file.rally
  if (rally.length >= 5) return // V-12: ≤5 rally points
  const seq = rally.length > 0 ? Math.max(...rally.map((r) => r.seq)) + 1 : 0
  setState({
    file: {
      ...state.file,
      rally: [...rally, { seq, frame: 3, x: lat, y: lng, z: 0 }],
    },
    dirty: true,
    validated: null,
    uploadProgress: null,
  })
}

export function deleteRallyPoint(seq: number): void {
  pushUndo()
  setState({
    file: { ...state.file, rally: state.file.rally.filter((r) => r.seq !== seq) },
    dirty: true,
    validated: null,
    uploadProgress: null,
  })
}

// ---------------------------------------------------------------------------
// File-level mutations — load, save, reset, undo
// ---------------------------------------------------------------------------

export function loadMission(file: PlanMissionFile): void {
  setState({
    file,
    dirty: false,
    validated: null,
    uploadProgress: null,
    downloadResult: null,
    selectedWp: null,
    undoStack: [],
  })
}

export function newMission(name: string = 'untitled'): void {
  setState({
    file: emptyPlanMission(name),
    dirty: false,
    validated: null,
    uploadProgress: null,
    downloadResult: null,
    selectedWp: null,
    undoStack: [],
  })
}

/** Called after a successful save — updates the file with the catalog's
 * response (new id + version) and clears dirty. */
export function onSaved(file: PlanMissionFile): void {
  setState({ file, dirty: false, validated: null, uploadProgress: null })
}

export function setValidated(result: PlanValidationResult | null): void {
  setState({ validated: result })
}

export function setUploadProgress(progress: UploadProgress | null): void {
  setState({ uploadProgress: progress })
}

export function setDownloadResult(result: PlanStoreState['downloadResult']): void {
  setState({ downloadResult: result })
}

export function setSelectedWp(seq: number | null): void {
  setState({ selectedWp: seq })
}

export function undo(): void {
  if (state.undoStack.length === 0) return
  const prev = state.undoStack[state.undoStack.length - 1]
  setState({
    file: prev,
    undoStack: state.undoStack.slice(0, -1),
    dirty: true,
    validated: null,
    uploadProgress: null,
  })
}

// ---------------------------------------------------------------------------
// Helpers — derived selectors
// ---------------------------------------------------------------------------

/** The set of waypoint seqs with validation errors (for L4 wp-err rings). */
export function getErroredSeqs(): Set<number> {
  if (!state.validated || state.validated.valid) return new Set()
  return new Set(state.validated.errors.filter((e) => e.seq != null && e.seq > 0).map((e) => e.seq as number))
}

/** Convenience: the current file's fence (for L3 layer). */
export function getFence(): PlanGeofence {
  return state.file.geofence
}

/** Convenience: the current file's waypoints (for L4 layer). */
export function getWaypoints(): PlanWaypoint[] {
  return state.file.waypoints
}

/** Convenience: the current file's rally (for L7 layer). */
export function getRally(): PlanRally[] {
  return state.file.rally
}
