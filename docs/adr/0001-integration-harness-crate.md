# ADR 0001: Integration harness as a workspace member

**Status**: Accepted (v0.1)
**Context**: The verification plan (spec §10.3) needs end-to-end tests that drive the
real `sitsim-cli` binary against a fake PX4 HIL client (codec-level, no PX4 binary
needed) plus the I-1 driver script against real PX4. The repo layout in spec §15 lists
`tests/` as "integration crate (10.3) + harness" without specifying workspace
membership.

**Decision**: `tests/` is a full workspace member (`sitsim-integration`, `publish =
false`) depending on `sitsim-mavlink` and tokio. The fake-PX4 client
(`tests/tests/fake_px4_boot.rs`) reuses the production codec instead of a parallel
Python implementation, which means the golden vectors and the client agree by
construction. `tests/run_i1.sh` remains a plain script (it must orchestrate processes
and PX4 itself, and asserts using an independent pymavlink oracle — deliberately NOT
the same codec as the product, so a codec bug cannot hide).

**Consequences**: `cargo test --workspace` includes integration tests (fast, no PX4
needed); real-PX4 cases stay behind `bash tests/run_i1.sh`, which CI runs as a separate
step. One deviation from the §15 sketch: a 9th workspace member exists.
