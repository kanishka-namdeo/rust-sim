/**
 * Bounded ring buffer for telemetry time-series (M8, T-A3).
 *
 * Spec: docs/GCS_V2_SPEC.md §9.1 — ring buffers (tracks 1800, strips 600,
 * events 200). One shared class used by the telemetry store for vehicle
 * tracks (per-vehicle [lon,lat] ring) and per-vehicle strip-chart samples
 * (alt/battery/speed/att key → {t, v} samples).
 *
 * No React state — module-level only. The store exposes ring accessors
 * `getTrack(i)` and `getStrip(i, key)` that return a snapshot copy.
 */

export class RingBuffer<T> {
  private buf: (T | undefined)[]
  private head = 0 // next write index
  private _size = 0
  readonly capacity: number

  constructor(capacity: number) {
    this.capacity = Math.max(1, Math.floor(capacity))
    this.buf = new Array<T | undefined>(this.capacity)
  }

  /** Push a sample; drops the oldest when full. */
  push(v: T): void {
    this.buf[this.head] = v
    this.head = (this.head + 1) % this.capacity
    if (this._size < this.capacity) this._size++
  }

  /** Current element count (≤ capacity). */
  get size(): number {
    return this._size
  }

  /** Returns the samples in chronological order (oldest first, newest last). */
  snapshot(): T[] {
    const out: T[] = new Array(this._size)
    if (this._size < this.capacity) {
      // Not yet wrapped: 0..size-1 are populated, head points to next slot.
      for (let i = 0; i < this._size; i++) out[i] = this.buf[i] as T
    } else {
      // Wrapped: head is the oldest, head-1 (mod cap) is the newest.
      for (let i = 0; i < this.capacity; i++) {
        out[i] = this.buf[(this.head + i) % this.capacity] as T
      }
    }
    return out
  }

  /** Most recent sample, or null if empty. */
  last(): T | null {
    if (this._size === 0) return null
    const lastIdx = (this.head - 1 + this.capacity) % this.capacity
    return this.buf[lastIdx] ?? null
  }

  /** Clear all samples. */
  reset(): void {
    this.buf = new Array<T | undefined>(this.capacity)
    this.head = 0
    this._size = 0
  }
}
