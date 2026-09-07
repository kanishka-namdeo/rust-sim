#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

//! sitsim-core: 6-DOF quadrotor-X dynamics, quaternion propagation, and the
//! PCG64 RNG (SPEC §2.2, §5, §8.1).
//!
//! This crate is `no_std` (floating-point math via libm; see ADR-001): no
//! allocation, no I/O, no wall-clock reads — the determinism contract of
//! SPEC §8.1 starts here.

pub mod dynamics;
pub mod quat;
pub mod rng;

pub use dynamics::{
    QuadDynamics, QuadParams, State, StepInput, MOTOR_SPIN, contact_fast_root, stable_substeps,
};
pub use quat::{quat_mul, quat_normalize, quat_norm, quat_to_euler, quat_to_rot};
pub use rng::{Pcg64, RngStreams};
