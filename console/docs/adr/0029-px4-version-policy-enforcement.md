# ADR-0029: PX4 version policy — hard reject on version mismatch

- Status: Accepted
- Date: 2026-09-09
- Owning milestone: M1 (Plan View MVP — the G-0 gate is M1-blocking)
- Supersedes: none
- Related: GCS_SPEC.md §3.3 (PX4 Version Policy), §5.1 AC-5.1.4
  (upload returns HTTP 426 on version mismatch), §9 G-0 (version-check
  gate), §11 R-8 (PX4 v1.18 release risk), §12 Q-11 (this ADR);
  ADR-0019 (mission file carries `px4_version` field); existing ADRs
  0010, 0011, 0015 (PX4 v1.16.2 dialect drift)

## Context

GCS_SPEC.md §3.3 (added in v0.2) commits RustSim GCS v1 to PX4
Autopilot v1.16.2. The rationale, grounded in 2026 research
(quad-drone-lab.co.kr weekly briefing Jul 2026: PX4 v1.18 entering
beta), is that every protocol claim in the existing ADRs (0010, 0011,
0015 — see `sim/docs/PROTOCOL.md`) was captured live against v1.16.2
and may not hold against v1.18. The spec left the *enforcement*
mechanism open (GCS_SPEC.md §12 Q-11): hard reject (HTTP 426) vs
warning + allow.

The decision matters because it determines what happens when an
operator clones PX4 at v1.18 (or any non-v1.16.2 version), starts a
SITL instance, and tries to drive it with RustSim GCS v1. The two
options have very different failure modes:

1. **Hard reject** — the upload is refused before any MAVLink mission
   items are sent. The operator gets a clear error: "vehicle 0 reports
   PX4 v1.18.0; RustSim GCS v1 requires v1.16.2." The operator must
   downgrade PX4 or wait for RustSim GCS v1.1. No silent breakage.

2. **Warning + allow** — the upload proceeds, but the operator sees a
   warning toast: "vehicle 0 reports PX4 v1.18.0; RustSim GCS v1 is
   tested against v1.16.2 — proceed at your own risk." If the upload
   fails mid-protocol (because v1.18's `MISSION_ITEM_INT` layout
   differs), the operator gets a confusing `MISSION_ACK: denied` and
   has to debug. If the upload succeeds but a downstream flight
   behavior differs (because v1.18's EKF2 handles sensor noise
   differently), the operator may not even notice until the vehicle
   does something unexpected.

The existing RustSim ADRs document three concrete cases where v1.16.2
diverges from the MAVLink `common.xml` spec — cases that only live
capture revealed and that a "warning + allow" policy would silently
break against v1.18:

- **ADR-0010**: `DO_SET_MODE` takes decomposed `param2`/`param3`, not
  the packed mode word. v1.18 may have reverted to the packed layout.
- **ADR-0011**: PX4 v1.16 sends per-motor normalized thrust [0,1],
  not the jMAVSim-era [-1,1]. v1.18 may have changed the range.
- **ADR-0015**: `HIL_ACTUATOR_CONTROLS` is packed with size-sorted
  core fields (`flags@8, controls@16, mode@80`), not the official
  extension layout. v1.18 may have aligned with `common.xml`.

These are not theoretical. Each was captured against real PX4 v1.16.2
and each would silently break against a different PX4 version. A GCS
that "warns and allows" is shipping a footgun.

## Decision

**Hard reject.** The `:8300` mission catalog server refuses to upload
a mission to a vehicle whose reported PX4 version differs from the
pinned v1.16.2. The rejection happens before any MAVLink mission
items are sent; the vehicle's PX4 mission pool is not modified. The
HTTP response is **426 Upgrade Required** with a JSON body:

```json
{
  "error": {
    "code": "PX4_VERSION_MISMATCH",
    "message": "vehicle 0 reports PX4 v1.18.0; RustSim GCS v1 requires v1.16.2",
    "details": {
      "vehicle_id": 0,
      "reported_version": "v1.18.0",
      "required_version": "v1.16.2",
      "doc_url": "https://github.com/kanishka-namdeo/rust-sim/blob/main/docs/GCS_SPEC.md#33-px4-version-policy"
    }
  }
}
```

The Plan View's upload flow (GCS_SPEC.md §8.1 step 12, error path C)
catches the 426, displays the toast: `"Upload rejected: vehicle 0
reports PX4 v1.18.0, RustSim GCS v1 requires v1.16.2 (HTTP 426)"`, and
does not advance any of the three upload progress bars.

### How the version is detected

The version check uses the **PX4 autopilot version string** reported
via `AUTOPILOT_VERSION` MAVLink message, which PX4 sends in response
to `MAV_CMD_REQUEST_AUTOPILOT_VERSION` (command 183). The fleet
manager (`:8400`) already maintains a `VehicleState` struct (per
ADR-0017) that includes the autopilot version, queried once at
vehicle boot. The `:8300` catalog server queries `:8400`'s
`GET /api/vehicles/{i}` endpoint to read `state.px4_version` before
starting the upload.

The version string is parsed with a simple regex
(`^v?(\d+)\.(\d+)\.(\d+)`) into a `(major, minor, patch)` tuple. The
check is **exact match** against `(1, 16, 2)`. A v1.16.2-rc1 or
v1.16.2+dirty build is **rejected** (the `-rc1` and `+dirty` suffixes
indicate unreleased builds that may differ from the tagged release).

### The G-0 gate

G-0 (GCS_SPEC.md §9, new in v0.2) is the verification gate for this
ADR. It asserts:

1. The PX4 build directory's `git describe --tags` output matches
   `v1.16.2` exactly. The harness runs in the RustSim sandbox where
   PX4 is cloned at `--branch v1.16.2`; the `sim/px4-version` and
   `fleet/px4-version` files record the SHA.
2. `:8300` rejects an upload to a vehicle reporting a different
   version with HTTP 426 and the `PX4_VERSION_MISMATCH` error code.
   The harness uses a mock `:8400` that returns a `px4_version` of
   `v1.18.0` for vehicle 0 and asserts the 426 response.
3. `:8300` accepts an upload to a vehicle reporting `v1.16.2`. The
   harness uses a mock `:8400` that returns `v1.16.2` and asserts
   the upload proceeds (the harness does not actually fly the
   mission — that's G-3's job).

The G-0 harness is a single-invocation script
(`console/tests/run_g0_version.sh`, est. 10 s) that starts the mock
`:8400`, starts `:8300`, runs the two assertions, and tears down. No
real PX4 is needed for G-0 (it tests the version-check logic, not the
MAVLink protocol); real PX4 is needed for G-3 (upload ack).

### Where the check lives

The version check is in `:8300`'s upload handler, not in `:8400` or
the console. Rationale:

- **`:8300` is the catalog owner.** It owns the mission file (which
  carries `px4_version` per ADR-0019), the upload path, and the
  rejection logic. Keeping the check here means the catalog is
  self-contained: an operator using `curl` against `:8300` directly
  (no console) still gets the version gate.
- **`:8400` is the fleet manager.** It owns the vehicle state
  (including the `px4_version` field) but does not own the mission
  catalog. Putting the check in `:8400` would require `:8400` to
  know the catalog's required version — a reverse dependency.
- **The console is the UI.** It displays the rejection toast but does
  not enforce the policy. A console-side check would be bypassable
  by `curl`; the server-side check is not.

### The `px4_version` field on the mission file

Per ADR-0019, every mission TOML carries a `px4_version` field
(defaulting to `v1.16.2`). This field is informational in v1 — the
upload check uses the *vehicle's* reported version, not the
mission's recorded version. In a future v1.1, the catalog may
refuse to *load* a mission whose `px4_version` differs from the
pinned version (so an operator cannot even open a v1.18-authored
mission in v1). For v1, the check is upload-time only.

### Operator escape hatch (documented, not UI-exposed)

An operator who genuinely wants to test against v1.18 (e.g. to
capture dialect drift for a future ADR) can bypass the check by
setting `RSIM_ALLOW_UNPINNED_PX4=1` in the `:8300` server's
environment. This downgrades the rejection to a warning: the upload
proceeds, but the response includes a `"warning"` field with the
same details. This escape hatch is **not exposed in the UI** — the
operator must set the env var and restart `:8300`. It exists so
that the RustSim core team can run v1.18 capture sessions without
patching the code; it is not a feature for end users.

## Consequences

### Positive

- **No silent breakage.** An operator who clones v1.18 and tries to
  use RustSim GCS v1 gets a clear, actionable error in <1 s (the
  version query is one REST round-trip). No debugging `MISSION_ACK:
  denied` messages.
- **Consistent with the existing ADR discipline.** ADRs 0010, 0011,
  0015 establish that RustSim captures protocol facts live and
  documents them. A "warning + allow" policy would undermine that
  discipline by allowing un-captured protocol variants. Hard reject
  enforces the discipline.
- **The version policy is a documented decision, not a surprise.**
  The doc_url in the error response points to GCS_SPEC.md §3.3,
  which explains *why* the rejection happens. The operator is not
  left guessing.
- **The escape hatch is explicit.** `RSIM_ALLOW_UNPINNED_PX4=1` is
  a documented env var, not a hidden flag. The RustSim core team
  can use it for capture sessions; an end user who finds it and
  uses it does so at their own risk.

### Negative

- **Operators on v1.18 cannot use RustSim GCS v1.** This is the
  intended behavior, but it will generate support requests ("why
  doesn't it work with my v1.18 SITL?"). Mitigation: the README and
  the GCS_SPEC.md §3.3 page state the policy clearly; the error
  toast includes the doc_url.
- **The version query adds one REST round-trip to every upload.**
  `:8300` calls `:8400`'s `GET /api/vehicles/{i}` before starting
  the MAVLink upload. This is ~5 ms on localhost; negligible. The
  query is cached for 5 s (so a rapid re-upload to the same vehicle
  does not re-query), but a vehicle that restarts mid-session gets
  a fresh query.
- **The `AUTOPILOT_VERSION` message may not be available
  immediately at vehicle boot.** PX4 SITL sends `AUTOPILOT_VERSION`
  in response to `MAV_CMD_REQUEST_AUTOPILOT_VERSION`, but the fleet
  manager may not have queried it yet when the operator clicks
  Upload. Mitigation: the `:8400` `GET /api/vehicles/{i}` endpoint
  triggers a lazy query if `state.px4_version` is `None`, blocking
  for up to 2 s; if the version is still `None` after 2 s, the
  upload is rejected with HTTP 503 and the message "vehicle 0 PX4
  version not yet available; retry in a few seconds".

### Neutral

- **The check is at upload time, not at catalog load time.** An
  operator can save a v1.18-tagged mission in the catalog (the
  `px4_version` field is informational); the check fires only when
  they try to upload it to a vehicle. This is the v1 behavior; v1.1
  may tighten it (see above).
- **The check is on the vehicle's reported version, not the
  mission's recorded version.** An operator who authors a mission
  against v1.16.2 and later tries to upload it to a v1.18 vehicle
  gets the rejection based on the vehicle's version, not the
  mission's. This is correct — the vehicle's version is what
  determines protocol compatibility.

## Alternatives considered

### Warning + allow (rejected)

The upload proceeds; the operator sees a warning toast. **Rejected**
because:
- The existing ADRs (0010, 0011, 0015) document three concrete
  protocol divergences that would silently break. A "warning +
  allow" policy is a footgun: the operator clicks through the
  warning, the upload appears to succeed (the MAVLink ACKs may
  still come back), and the failure surfaces only mid-flight when
  the vehicle does something unexpected.
- Operators under time pressure routinely click through warnings.
  The QGC user community's most-requested feature list (PX4 Discuss
  forum, Jun 2018, still open) includes "stop showing me calibration
  warnings I've already dismissed" — evidence that operators dismiss
  warnings reflexively.
- The cost of a hard reject is low (the operator downgrades PX4 or
  waits for v1.1); the cost of a silent mid-flight failure is high
  (debugging time, lost mission, potential simulator state
  corruption).

### No version check (rejected)

Trust the operator to read the docs. **Rejected** because the docs
are not in the operator's critical path — the operator clones PX4,
runs `make px4_sitl_default`, starts the fleet, opens the console,
and clicks Upload. None of those steps surface the version policy
unless the console enforces it.

### Compile-time version check (rejected)

Bake the PX4 version into the RustSim binary at compile time; refuse
to start `:8300` if the build-time PX4 version differs from v1.16.2.
**Rejected** because the build-time PX4 version (the one on the
machine that compiled RustSim) is not the same as the runtime PX4
version (the one the operator is running SITL against). An operator
may compile RustSim on a CI runner with v1.16.2 installed and run
it against a v1.18 SITL on their laptop. The runtime check is the
correct one.

### Per-feature version negotiation (rejected)

Negotiate each feature individually: "this mission uses
`MISSION_ITEM_INT`, which v1.18 supports, so upload is allowed;
this mission uses the `DO_SET_MODE` packed-word layout, which v1.18
may not support, so that command is rejected." **Rejected** because
it requires a feature-compatibility matrix per PX4 version, which
is exactly the kind of un-captured knowledge the existing ADR
discipline exists to prevent. v1 does not have the ADR coverage to
support per-feature negotiation; v1.1 or v2 may, after capture
sessions against v1.18 produce new ADRs.

## Verification

- **Unit:** `fleet-mission` (or `fleet-cli` per ADR-0027) has a
  `version_check` test module that:
  - Mocks `:8400` returning `px4_version = "v1.16.2"`; asserts the
    upload proceeds (HTTP 200 from the version-check stage).
  - Mocks `:8400` returning `px4_version = "v1.18.0"`; asserts the
    upload is rejected with HTTP 426 and `PX4_VERSION_MISMATCH`.
  - Mocks `:8400` returning `px4_version = "v1.16.2-rc1"`; asserts
    rejection (the `-rc1` suffix fails the exact match).
  - Mocks `:8400` returning `px4_version = "v1.16.2+dirty"`; asserts
    rejection (the `+dirty` suffix fails the exact match).
  - Mocks `:8400` returning `px4_version = null` after 2 s timeout;
    asserts HTTP 503 with "version not yet available".
  - Sets `RSIM_ALLOW_UNPINNED_PX4=1`; mocks `:8400` returning
    `v1.18.0`; asserts the upload proceeds with a `"warning"` field
    in the response.
- **Integration:** G-0 (GCS_SPEC.md §9) exercises the full path: mock
  `:8400`, real `:8300`, two assertions (accept v1.16.2, reject
  v1.18.0), teardown. 10 s total.
- **Regression:** the existing I-1/I-2/F-1/F-2 harnesses (which run
  against real PX4 v1.16.2) continue to pass — the version check
  sees `v1.16.2` and allows the upload. No regression.

## References

- GCS_SPEC.md §3.3 (PX4 Version Policy — the spec section this ADR
  implements), §5.1 AC-5.1.4 (HTTP 426 on version mismatch), §9 G-0
  (version-check gate), §11 R-8 (PX4 v1.18 release risk), §12 Q-11
  (this ADR's open question, now resolved)
- ADR-0010 (`DO_SET_MODE` param layout — v1.16.2 divergence)
- ADR-0011 (per-motor normalized thrust — v1.16.2 divergence)
- ADR-0015 (`HIL_ACTUATOR_CONTROLS` size-sorted layout — v1.16.2
  divergence)
- ADR-0019 (mission file format — the `px4_version` field on
  `MissionFile`)
- ADR-0027 (`:8300` server shape — where the version-check code
  lives)
- PX4 v1.16 `AUTOPILOT_VERSION` message reference —
  https://docs.px4.io (PX4 responds to `MAV_CMD_REQUEST_AUTOPILOT_VERSION`
  command 183)
- Quad-Drone-Lab, "PX4 weekly briefing" (Jul 20, 2026) —
  https://quad-drone-lab.co.kr (grounds the v1.18-beta risk)
- HTTP 426 Upgrade Required status code —
  https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/426
  (semantically correct: the server refuses to perform the action
  using the current protocol version, and the client should upgrade)
