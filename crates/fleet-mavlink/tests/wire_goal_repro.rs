//! Wire-level repro (F-2 no-flight root cause): what does the 20 Hz setpoint
//! pump actually put on the wire for a NED goal with negative z (up)?
//!
//! Spawns a real link against a fake PX4 onboard endpoint, sets the goal
//! (0, 40, -10), and decodes SET_POSITION_TARGET_LOCAL_NED frames off the wire
//! using fleet-mavlink's own decoder. The position must march toward the goal
//! in NED: y -> 40, z -> -10. If z ramps POSITIVE the goal/altitude sign is
//! flipped somewhere in the link chain (the F-2 live-capture symptom:
//! PX4 logged trajectory_setpoint z = +10.008 for a -10 m target).

use std::time::Duration;

use fleet_mavlink::frame::{crc_extra, Decoder};
use fleet_mavlink::messages::SetPositionTargetLocalNed;
use fleet_mavlink::{ids, spawn_link, LinkConfig, SetpointGoal};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pump_puts_goal_z_on_the_wire_in_ned() {
    // Fake PX4 onboard endpoint: instance 9 -> link binds 14549, sends to 14589.
    let fake_px4 = std::net::UdpSocket::bind("127.0.0.1:14589").expect("bind fake px4");
    fake_px4
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();

    let (handle, _ev) = spawn_link(LinkConfig::for_instance(9)).expect("spawn link");
    // Real flow: HoldAt(anchor) activates the stream, then set_goal retargets it.
    handle.shared.hold_at([0.002, 0.001, 0.008], 0.017);
    handle.shared.set_goal(SetpointGoal {
        position: [0.0, 40.0, -10.0],
        yaw: 0.02,
    });

    let mut dec = Decoder::new();
    let mut samples: Vec<[f32; 3]> = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_millis(2600);
    let mut buf = [0u8; 2048];
    while std::time::Instant::now() < deadline {
        match fake_px4.recv(&mut buf) {
            Ok(n) => {
                dec.push(&buf[..n]);
                while let Some(frame) = dec.next_frame() {
                    if frame.msgid == ids::SET_POSITION_TARGET_LOCAL_NED {
                        if let Some(m) = SetPositionTargetLocalNed::unpack(&frame.payload) {
                            samples.push([m.x, m.y, m.z]);
                        }
                    }
                }
            }
            Err(_) => { /* read timeout — loop */ }
        }
    }
    drop(handle);
    assert!(!samples.is_empty(), "no setpoint frames observed on the wire");
    let last = samples.last().unwrap();
    let first = samples.first().unwrap();
    println!("first setpoint: {:?}", first);
    println!("last  setpoint: {:?}", last);
    println!("n={}", samples.len());

    // The pump steps per-axis at 4 m/s at 20 Hz: after ~2.6 s the z axis must
    // have arrived at the goal (-10) and y must be ~2/3 of the way to 40.
    // THE F-2 BUG ASSERT: z must go NEGATIVE (NED up), never +10.
    assert!(last[2] <= -9.5, "z never reached the goal: {}", last[2]);
    assert!(last[2] > -f32::INFINITY && last[2].is_finite(), "z not finite");
    assert!(last[1] >= 9.0, "y did not ramp toward 40: {}", last[1]);
    assert!((last[0] - 0.0).abs() < 0.5, "x drifted: {}", last[0]);
    // Monotone z: once past the goal, stay (no sign flip mid-stream).
    let zmin = samples.iter().map(|s| s[2]).fold(f32::INFINITY, f32::min);
    let zmax = samples.iter().map(|s| s[2]).fold(f32::NEG_INFINITY, f32::max);
    assert!(zmin >= -10.5 && zmax <= 0.5, "z out of envelope: [{zmin}, {zmax}]");
}

// Silence unused warnings for crc_extra (kept to document the decode path).
#[allow(dead_code)]
fn _doc() {
    let _ = crc_extra(84u32);
}
