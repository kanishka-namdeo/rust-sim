'use client'

/**
 * GCS v2 — First-run welcome + guided tour (UX spec §3).
 *
 * On the first visit (no localStorage["rsim.onboarded"]), show a centered
 * welcome dialog with "Take the 30s tour" or "Skip". The tour highlights
 * 3 zones (rail, command bar, telemetry column) with dimmed background +
 * cyan border + tooltip text. After the tour (or skip), auto-open the
 * cheat sheet and set the onboarded flag.
 */

import { useEffect, useState, type JSX } from 'react'
import { toggleOverlay } from '@/state/app-store'

const ONBOARDED_KEY = 'rsim.onboarded'
const TOUR_STEPS = [
  { zone: 'B', title: 'Left rail', text: 'Modes + panels live here. Click Plan to start a mission, Fleet C2 to see your vehicles.' },
  { zone: 'D', title: 'Command bar', text: 'Flight verbs here. Single-key: A = ARM, L = land, E = estop. Hold ARM for 400ms.' },
  { zone: 'C', title: 'Telemetry column', text: 'Live telemetry for the active vehicle. Click a vehicle chip (top) to switch.' },
] as const

export function OnboardingTour(): JSX.Element | null {
  const [showWelcome, setShowWelcome] = useState(false)
  const [tourStep, setTourStep] = useState<number>(-1)

  useEffect(() => {
    try {
      if (!localStorage.getItem(ONBOARDED_KEY)) {
        // eslint-disable-next-line react-hooks/set-state-in-effect -- legitimate first-run check (localStorage → state)
        setShowWelcome(true)
      }
    } catch {
      // localStorage unavailable
    }
  }, [])

  const finish = (): void => {
    try { localStorage.setItem(ONBOARDED_KEY, 'true') } catch { /* non-fatal */ }
    setShowWelcome(false)
    setTourStep(-1)
  }

  const startTour = (): void => {
    setShowWelcome(false)
    setTourStep(0)
  }

  const nextStep = (): void => {
    if (tourStep < TOUR_STEPS.length - 1) {
      setTourStep(tourStep + 1)
    } else {
      finish()
      // Auto-open cheat sheet after tour
      toggleOverlay('cheat')
    }
  }

  const skipTour = (): void => {
    finish()
  }

  // Welcome dialog
  if (showWelcome) {
    return (
      <div
        style={{
          position: 'fixed',
          inset: 0,
          zIndex: 200,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          background: 'rgba(0, 0, 0, 0.7)',
          backdropFilter: 'blur(4px)',
        }}
        role="dialog"
        aria-modal="true"
        aria-labelledby="welcome-title"
      >
        <div
          style={{
            maxWidth: 480,
            padding: 32,
            borderRadius: 'var(--rsim-radius-panel)',
            background: 'var(--rsim-surface-solid)',
            border: '1px solid var(--rsim-accent)',
            boxShadow: 'var(--rsim-shadow-overlay)',
            textAlign: 'center',
            fontFamily: 'var(--rsim-font-ui)',
            color: 'var(--rsim-text)',
          }}
        >
          <div style={{ fontSize: 24, fontWeight: 700, marginBottom: 8 }} id="welcome-title">
            Welcome to RustSim GCS
          </div>
          <div style={{ fontSize: 14, color: 'var(--rsim-text-dim)', marginBottom: 24, lineHeight: 1.6 }}>
            The single-screen operator surface for PX4 SITL.<br />
            Map-first — everything is on one canvas.<br />
            Single-key — A to arm, L to land, ? for shortcuts.<br />
            2 SITL vehicles are already connected and READY.
          </div>
          <div style={{ display: 'flex', gap: 12, justifyContent: 'center' }}>
            <button
              type="button"
              onClick={startTour}
              style={{
                background: 'var(--rsim-accent)',
                border: 'none',
                borderRadius: 'var(--rsim-radius-control)',
                color: '#0B0F14',
                fontSize: 14,
                fontWeight: 600,
                padding: '10px 20px',
                cursor: 'pointer',
              }}
            >
              Take the 30s tour →
            </button>
            <button
              type="button"
              onClick={skipTour}
              style={{
                background: 'transparent',
                border: '1px solid var(--rsim-border)',
                borderRadius: 'var(--rsim-radius-control)',
                color: 'var(--rsim-text-dim)',
                fontSize: 14,
                padding: '10px 20px',
                cursor: 'pointer',
              }}
            >
              Skip — I know my way
            </button>
          </div>
          <div style={{ marginTop: 16, fontSize: 11, color: 'var(--rsim-text-dim)' }}>
            Tip: press ? anytime to see all shortcuts
          </div>
        </div>
      </div>
    )
  }

  // Guided tour — highlight the current zone
  if (tourStep >= 0 && tourStep < TOUR_STEPS.length) {
    const step = TOUR_STEPS[tourStep]
    return (
      <div
        style={{
          position: 'fixed',
          inset: 0,
          zIndex: 200,
          pointerEvents: 'none',
        }}
      >
        {/* Dim background */}
        <div style={{ position: 'absolute', inset: 0, background: 'rgba(0, 0, 0, 0.6)' }} />

        {/* Highlight border around the zone */}
        <div
          style={{
            position: 'absolute',
            border: '2px solid var(--rsim-accent)',
            borderRadius: 'var(--rsim-radius-panel)',
            boxShadow: '0 0 0 4px rgba(34, 211, 238, 0.2), 0 0 20px rgba(34, 211, 238, 0.4)',
            ...getZoneBox(step.zone),
          }}
        />

        {/* Tooltip */}
        <div
          style={{
            position: 'absolute',
            left: '50%',
            bottom: 120,
            transform: 'translateX(-50%)',
            pointerEvents: 'auto',
            padding: '16px 24px',
            borderRadius: 'var(--rsim-radius-panel)',
            background: 'var(--rsim-surface-solid)',
            border: '1px solid var(--rsim-accent)',
            boxShadow: 'var(--rsim-shadow-overlay)',
            maxWidth: 400,
            textAlign: 'center',
          }}
        >
          <div style={{ fontSize: 12, fontWeight: 700, color: 'var(--rsim-accent)', textTransform: 'uppercase', letterSpacing: '0.08em', marginBottom: 6 }}>
            {step.title} · Step {tourStep + 1} of {TOUR_STEPS.length}
          </div>
          <div style={{ fontSize: 14, color: 'var(--rsim-text)', lineHeight: 1.5 }}>
            {step.text}
          </div>
          <div style={{ display: 'flex', gap: 8, justifyContent: 'center', marginTop: 12 }}>
            <button
              type="button"
              onClick={nextStep}
              style={{
                background: 'var(--rsim-accent)',
                border: 'none',
                borderRadius: 'var(--rsim-radius-control)',
                color: '#0B0F14',
                fontSize: 12,
                fontWeight: 600,
                padding: '6px 16px',
                cursor: 'pointer',
              }}
            >
              {tourStep < TOUR_STEPS.length - 1 ? 'Next →' : 'Done ✓'}
            </button>
            <button
              type="button"
              onClick={skipTour}
              style={{
                background: 'transparent',
                border: '1px solid var(--rsim-border)',
                borderRadius: 'var(--rsim-radius-control)',
                color: 'var(--rsim-text-dim)',
                fontSize: 12,
                padding: '6px 16px',
                cursor: 'pointer',
              }}
            >
              Skip
            </button>
          </div>
        </div>
      </div>
    )
  }

  return null
}

// Zone bounding boxes (matching the zone layout in OperationsCanvas)
function getZoneBox(zone: string): React.CSSProperties {
  switch (zone) {
    case 'B':
      return { left: 0, top: 48, width: 56, bottom: 96 }
    case 'D':
      return { left: 0, right: 0, bottom: 0, height: 96 }
    case 'C':
      return { right: 0, top: 48, width: 312, bottom: 96 }
    default:
      return {}
  }
}
