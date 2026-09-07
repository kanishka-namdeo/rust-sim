#!/usr/bin/env python3
"""I-2 diagnosis helper: EKF vs ground-truth vertical + aiding sources."""
import sys
import numpy as np
from pyulog import ULog

ULOG = sys.argv[1] if len(sys.argv) > 1 else \
    "/home/z/my-project/PX4-Autopilot/build/px4_sitl_default/instance_0/log/2026-09-07/13_26_08.ulg"

u = ULog(ULOG, None)
T = {}
for d in u.data_list:
    T.setdefault(d.name, d)  # first instance

lp = T["vehicle_local_position"]
gt = T["vehicle_local_position_groundtruth"]
tl = lp.data["timestamp"] / 1e6
tl -= tl[0]
z = lp.data["z"]
tg = gt.data["timestamp"] / 1e6
tg -= tg[0]
zg = gt.data["z"]

print("t      EKF z    truth z   diff")
for tt in [19.5, 23, 26, 30, 34, 40, 46, 52, 57, 60]:
    i = np.searchsorted(tl, tt)
    j = np.searchsorted(tg, tt)
    if i < len(tl) and j < len(tg):
        print(f"{tt:5.1f}  {z[i]:7.2f}  {zg[j]:7.2f}  {z[i]-zg[j]:7.2f}")
print(f"EKF z_min={z.min():.2f}  truth z_min={zg.min():.2f}")

for src in ["estimator_aid_src_baro_hgt", "estimator_aid_src_gnss_hgt"]:
    d = T.get(src)
    if d:
        t = d.data["timestamp"] / 1e6
        t -= t[0]
        inn = d.data.get("innovation")
        fused = d.data.get("fused")
        if inn is not None:
            print(f"{src}: n={len(t)} innovation range [{inn.min():.2f},{inn.max():.2f}] "
                  f"mean={inn.mean():.2f}  (fused at end: {bool(fused[-1]) if fused is not None else '?'})")
            for tt in [19, 30, 40, 50]:
                i = np.searchsorted(t, tt)
                if i < len(t):
                    print(f"  t={tt:3d}s innovation={inn[i]:7.2f}")

d = T.get("estimator_baro_bias")
if d:
    t = d.data["timestamp"] / 1e6
    t -= t[0]
    for k in sorted(d.data):
        if "vert" in k:
            v = d.data[k]
            print(f"baro_bias.{k}: start={v[0]:.2f} end={v[-1]:.2f} range [{v.min():.2f},{v.max():.2f}]")

d = T.get("sensor_baro")
if d:
    t = d.data["timestamp"] / 1e6
    t -= t[0]
    dt = np.diff(t)
    print(f"sensor_baro: n={len(t)} max gap {dt.max()*1e3:.0f} ms at t={t[np.argmax(dt)]:.1f}s, "
          f"gaps>250ms: {int(np.sum(dt > 0.25))}")
    for k in sorted(d.data):
        if "alt" in k:
            a = d.data[k]
            print(f"  {k}: first={a[0]:.2f} last={a[-1]:.2f} range [{a.min():.2f},{a.max():.2f}]")

d = T.get("sensor_gps")
if d:
    t = d.data["timestamp"] / 1e6
    t -= t[0]
    alts = [k for k in sorted(d.data) if "alt" in k.lower()]
    print("sensor_gps alt-ish fields:", alts)
    for k in alts:
        a = d.data[k]
        sc = 1e3 if abs(float(np.mean(a))) > 1e4 else 1.0
        print(f"  {k}: n={len(t)} first={a[0]/sc:.2f} last={a[-1]/sc:.2f} range [{a.min()/sc:.2f},{a.max()/sc:.2f}] (m)")
