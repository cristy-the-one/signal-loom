#!/usr/bin/env python3
"""Synthetic 10-minute hybrid hypercar bus.

This is not a capture. Message ids, checksums, and the lap are invented
for Signal Loom. The layout is a private bus: ECM, TCU, ABS, ESC, BCM,
cluster, and BMS. Payloads use an XOR checksum in byte 7 and a nibble
or byte counter. Cycle times are 10, 20, 100, and 1000 ms, with jitter.

Speed comes from a seeded longitudinal model (torque curve, gear ratio,
aero drag, rolling resistance, mass). A slow grade and a headwind move
the aero balance so a flat-out straight is not a dead line. RPM follows
road speed through the gear, with a torque cut on upshifts and a blip on
downshifts. Brake pressure is zero except during scheduled applies: a
100–200 ms rise, a constant hold, and a release. The lap is urban, several
corners, a chicane, one short top-speed straight, a second shorter
straight, and a pit.
"""

from __future__ import annotations

import math
import random
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "fixtures"
DURATION_S = 600.0
DT_S = 0.001

# Overall ratio, engine rpm / wheel rpm. 7th is the top-speed gear.
# Full-throttle drag and the torque curve meet under the rev limiter.
GEARS = [0.0, 12.8, 8.6, 6.3, 4.9, 4.05, 3.5, 3.15]
# Torque starts to fall here. The straight is geared to stay below it.
REV_LIMIT = 9050.0
WHEEL_RADIUS = 0.335
WHEELBASE = 2.70
TRACK = 1.64
MASS = 1390.0
G = 9.81
# 0.5 * Cd * A * rho, Cd 0.36, A 2.0 m^2.
CDA = 0.5 * 0.36 * 2.0 * 1.225
CRR = 0.012
MU = 1.40
IDLE = 980.0
DRIVE_EFF = 0.92

ECM_FAST = 0x100
ECM_SLOW = 0x101
TCU = 0x200
ABS = 0x300
ESC = 0x310
BCM = 0x400
CLUSTER = 0x500
BMS = 0x600
DIAG = 0x700

MESSAGES = [
    (ECM_FAST, 10_000, "ECM"),
    (TCU, 10_000, "TCU"),
    (ABS, 10_000, "ABS"),
    (ESC, 20_000, "ESC"),
    (ECM_SLOW, 100_000, "ECM"),
    (BCM, 100_000, "BCM"),
    (CLUSTER, 100_000, "IC"),
    (BMS, 100_000, "BMS"),
    (DIAG, 1_000_000, "DIAG"),
]

DTC_S = 125.0
COUNTER_FAULT_S = 200.0
ECM_GAP = (250.0, 250.08)
BUS_OFF = (410.0, 410.045)
CHECKSUM_FAULT_S = 460.0

# (t1, desired km/h, throttle cap, peak lateral g). Lateral g is a sine
# bump across the leg so a corner builds and releases. Straights are 0 g.
# The 420 km/h leg is a throttle demand, not a speed the car can reach.
# Aero drag sets the top speed, and only for the last part of that leg.
_LEG_ROWS = [
    (8.0, 0.0, 0.0, 0.0),
    (20.0, 40.0, 0.42, 0.0),
    (32.0, 16.0, 0.22, 0.48),
    (48.0, 58.0, 0.48, 0.0),
    (62.0, 20.0, 0.22, -0.58),
    (78.0, 46.0, 0.38, 0.22),
    (94.0, 24.0, 0.20, -0.42),
    (128.0, 130.0, 0.72, 0.0),
    (148.0, 72.0, 0.32, 0.82),
    (168.0, 155.0, 0.78, -0.28),
    (212.0, 195.0, 0.62, 0.12),
    (224.0, 110.0, 0.35, 0.95),
    (236.0, 125.0, 0.45, -1.05),
    (247.0, 95.0, 0.32, 0.72),
    (293.0, 420.0, 1.0, 0.0),
    (312.0, 78.0, 0.30, 0.88),
    (338.0, 215.0, 0.78, 0.0),
    (356.0, 64.0, 0.28, -1.05),
    (378.0, 150.0, 0.70, 0.35),
    (396.0, 58.0, 0.26, -0.78),
    (428.0, 185.0, 0.82, -0.18),
    (448.0, 70.0, 0.30, 0.92),
    (472.0, 125.0, 0.55, 0.0),
    (492.0, 48.0, 0.24, -0.62),
    (522.0, 100.0, 0.50, 0.40),
    (544.0, 32.0, 0.28, 0.12),
    (572.0, 8.0, 0.15, 0.0),
    (600.0, 0.0, 0.0, 0.0),
]
# Explicit applies only. Rise and release are 100–200 ms. The hold is one
# pressure, then the trace returns to exactly zero.
_BRAKE_ROWS = [
    (18.6, 0.14, 1.4, 0.16, 12.0),
    (46.4, 0.15, 1.8, 0.18, 16.0),
    (76.2, 0.12, 1.5, 0.16, 14.0),
    (126.4, 0.16, 2.2, 0.18, 26.0),
    (146.2, 0.14, 1.6, 0.16, 22.0),
    (212.4, 0.15, 1.8, 0.18, 32.0),
    (224.6, 0.13, 1.4, 0.16, 24.0),
    (236.8, 0.14, 1.6, 0.16, 28.0),
    (293.2, 0.16, 6.4, 0.20, 64.0),
    (336.2, 0.15, 2.6, 0.18, 40.0),
    (376.4, 0.14, 2.0, 0.16, 30.0),
    (426.2, 0.15, 2.4, 0.18, 34.0),
    (470.4, 0.13, 2.2, 0.18, 26.0),
    (520.2, 0.16, 2.0, 0.18, 18.0),
    (542.0, 0.14, 1.6, 0.16, 12.0),
]


def clamp(value: float, lo: float, hi: float) -> float:
    return max(lo, min(hi, value))


def smoothstep(value: float) -> float:
    value = clamp(value, 0.0, 1.0)
    return value * value * (3.0 - 2.0 * value)


def lag(current: float, target: float, tau_s: float) -> float:
    alpha = 1.0 - math.exp(-DT_S / tau_s)
    return current + (target - current) * alpha


def legs() -> list[tuple[float, float, float, float, float]]:
    built = []
    t0 = 0.0
    for t1, speed, cap, lat in _LEG_ROWS:
        built.append((t0, t1, speed, cap, lat))
        t0 = t1
    return built


LEGS = legs()


def leg_at(t_s: float) -> tuple[float, float, float, float, float]:
    for leg in LEGS:
        if t_s < leg[1]:
            return leg
    return LEGS[-1]


def lat_target(t_s: float, leg: tuple[float, float, float, float, float]) -> float:
    peak = leg[4]
    if abs(peak) < 0.01:
        return 0.0
    dur = max(leg[1] - leg[0], 0.001)
    phase = clamp((t_s - leg[0]) / dur, 0.0, 1.0)
    return peak * math.sin(math.pi * phase)


def _straight_gust(t_s: float, rise_s: float) -> float:
    """1 on the top-speed straight, then back to 0 after the braking zone."""
    if t_s <= 276.0 or t_s >= 306.0:
        return 0.0
    level = smoothstep(min((t_s - 276.0) / rise_s, 1.0))
    if t_s > 294.0:
        level *= 1.0 - smoothstep(min((t_s - 294.0) / 10.0, 1.0))
    return level


def road_slope(t_s: float) -> float:
    """Uphill is positive. The top-speed straight runs onto a hill."""
    grade = 0.003 * math.sin(2.0 * math.pi * (t_s - 30.0) / 80.0)
    grade += 0.042 * _straight_gust(t_s, 4.0)
    return grade


def headwind_mps(t_s: float) -> float:
    """Positive is a headwind. A gust arrives once the straight is at speed."""
    wind = 0.7 * math.sin(2.0 * math.pi * (t_s - 80.0) / 50.0)
    wind += 4.2 * _straight_gust(t_s, 5.0)
    return wind


def brake_pressure_bar(t_s: float) -> float:
    """One smooth apply, a constant hold, a smooth release, or exactly zero."""
    for t0, rise, hold, fall, peak in _BRAKE_ROWS:
        end = t0 + rise + hold + fall
        if t_s < t0 or t_s >= end:
            continue
        u = t_s - t0
        if u < rise:
            return peak * smoothstep(u / rise)
        if u < rise + hold:
            return peak
        return peak * (1.0 - smoothstep((u - rise - hold) / fall))
    return 0.0


def engine_torque_nm(rpm: float) -> float:
    """Wide full-throttle curve. Peak in the midrange, falling toward redline."""
    if rpm < 900:
        return 140.0
    if rpm < 2000:
        return 140.0 + 420.0 * (rpm - 900) / 1100.0
    if rpm < 4200:
        return 560.0 + 250.0 * (rpm - 2000) / 2200.0
    if rpm < 6800:
        return 810.0 - 30.0 * (rpm - 4200) / 2600.0
    if rpm < 8600:
        return 780.0 - 300.0 * (rpm - 6800) / 1800.0
    return 460.0


def motor_nm(speed_kmh: float) -> float:
    if speed_kmh < 70:
        return 320.0
    if speed_kmh < 240:
        return 320.0 * (1.0 - (speed_kmh - 70) / 220.0)
    return 35.0


def kinematic_rpm(speed_mps: float, gear: int) -> float:
    if gear <= 0:
        return 0.0
    wheel_rpm = speed_mps / (2.0 * math.pi * WHEEL_RADIUS) * 60.0
    return wheel_rpm * GEARS[gear]


def set_bits(data: bytearray, start: int, length: int, raw: int) -> None:
    mask = (1 << length) - 1
    raw &= mask
    for bit in range(length):
        if (raw >> bit) & 1:
            index = start + bit
            data[index // 8] |= 1 << (index % 8)


def set_signed(data: bytearray, start: int, length: int, raw: int) -> None:
    if raw < 0:
        raw = (1 << length) + raw
    set_bits(data, start, length, raw)


def finish(data: bytearray, counter: int, counter_start: int, counter_len: int, bad: bool) -> bytes:
    set_bits(data, counter_start, counter_len, counter & ((1 << counter_len) - 1))
    checksum = 0
    for byte in data[:7]:
        checksum ^= byte
    if bad:
        checksum ^= 0xFF
    data[7] = checksum & 0xFF
    return bytes(data)


class Bus:
    def __init__(self) -> None:
        self.rng = random.Random(0x51_6C_4F_4D)
        self.speed_mps = 0.0
        self.gear = 0
        self.throttle = 0.0
        self.brake_bar = 0.0
        self.torque = 0.0
        self.torque_scale = 1.0
        self.rpm = 0.0
        self.steer = 0.0
        self.lat_g = 0.0
        self.coolant = 38.0
        self.oil = 28.0
        self.oil_pressure = 1.2
        self.fuel = 62.0
        self.soc = 74.0
        self.pack_v = 408.0
        self.pack_a = 0.0
        self.cell_temp = 24.0
        self.load_avg = 0.0
        self.abs_active = False
        self.esc_active = False
        self.mil = 0
        self.dtc_count = 0
        self.shift_kind = ""
        self.shift_elapsed = 0.0
        self.shift_from_rpm = IDLE
        self.shift_at = -10.0
        self.shift_lock_until = 0.0
        self.counters = {msg[0]: 0 for msg in MESSAGES}
        self.counter_fault_used = False
        self.checksum_fault_used = False
        self.t_s = 0.0

    def step(self) -> None:
        self._advance_shift()
        if self.t_s < 8.0:
            self.throttle = 0.0
            self.brake_bar = 0.0
        else:
            self.brake_bar = brake_pressure_bar(self.t_s)
            self._throttle(leg_at(self.t_s))
            self._maybe_shift()
        self._integrate()
        self._track_rpm()
        self._chassis(leg_at(self.t_s))
        self._thermal()
        if self.t_s >= DTC_S:
            self.mil = 1
            self.dtc_count = 1
        self.t_s += DT_S

    def _throttle(self, leg: tuple[float, float, float, float, float]) -> None:
        desired = leg[2]
        cap = leg[3]
        speed_kmh = self.speed_mps * 3.6
        if self.brake_bar > 0.8:
            target = 0.0
        else:
            err = desired - speed_kmh
            cruise = self._cruise_throttle()
            if err > 12.0:
                target = cap
            elif err > -1.5:
                blend = clamp(err / 12.0, 0.0, 1.0)
                target = min(cap, cruise + (cap - cruise) * blend)
            else:
                target = 0.0
        step = 4.5 * DT_S
        self.throttle += clamp(target - self.throttle, -step, step)
        self.throttle = clamp(self.throttle, 0.0, 1.0)

    def _cruise_throttle(self) -> float:
        if self.gear <= 0 or self.speed_mps < 1.5:
            return 0.12
        drag = CDA * self.speed_mps * self.speed_mps + CRR * MASS * G
        need = drag * WHEEL_RADIUS
        kin = max(kinematic_rpm(self.speed_mps, self.gear), 1200.0)
        avail = (engine_torque_nm(kin) + motor_nm(self.speed_mps * 3.6)) * GEARS[self.gear] * DRIVE_EFF
        if avail < 1.0:
            return 0.35
        return clamp(need / avail, 0.0, 0.85)

    def _maybe_shift(self) -> None:
        if self.t_s < self.shift_lock_until or self.shift_kind:
            return
        if self.gear == 0:
            self.gear = 1
            self.shift_at = self.t_s
            self.shift_lock_until = self.t_s + 0.45
            return
        braking = self.brake_bar > 20.0
        if braking and self.gear > 1 and self.rpm < 5600:
            best = self.gear
            for gear in range(self.gear - 1, 0, -1):
                nxt = kinematic_rpm(self.speed_mps, gear)
                if nxt > 7200:
                    break
                best = gear
                if nxt >= 5600:
                    break
            if best < self.gear:
                self._begin_shift(best, "down")
            return
        if not braking and self.gear < 7 and self.throttle > 0.12:
            limit = 7850.0 if self.throttle > 0.55 else 4400.0
            nxt = kinematic_rpm(self.speed_mps, self.gear + 1)
            if self.rpm > limit and nxt < self.rpm - 350:
                self._begin_shift(self.gear + 1, "up")
                return
        if self.gear > 1 and self.rpm < 2800 and self.throttle < 0.55 and self.speed_mps > 1.2:
            nxt = kinematic_rpm(self.speed_mps, self.gear - 1)
            if 1400.0 < nxt < 8000.0:
                self._begin_shift(self.gear - 1, "down")

    def _begin_shift(self, gear: int, kind: str) -> None:
        self.shift_from_rpm = self.rpm
        self.gear = gear
        self.shift_kind = kind
        self.shift_elapsed = 0.0
        self.shift_at = self.t_s
        self.shift_lock_until = self.t_s + (0.55 if kind == "up" else 0.62)

    def _advance_shift(self) -> None:
        if not self.shift_kind:
            self.torque_scale = 1.0
            return
        dur = 0.11 if self.shift_kind == "up" else 0.16
        self.shift_elapsed += DT_S
        if self.shift_kind == "up":
            self.torque_scale = 0.0 if self.shift_elapsed < 0.08 else clamp((self.shift_elapsed - 0.08) / 0.03, 0.0, 1.0)
        else:
            self.torque_scale = 0.30
        if self.shift_elapsed >= dur:
            self.shift_kind = ""
            self.torque_scale = 1.0

    def _integrate(self) -> None:
        speed_kmh = self.speed_mps * 3.6
        if self.t_s < 8.0 or self.gear <= 0:
            eng = 0.0
            mot = 0.0
            drive = 0.0
        else:
            rpm = self.rpm if self.rpm > 400 else 900.0
            limit = 1.0
            if rpm > REV_LIMIT:
                limit = clamp(1.0 - (rpm - REV_LIMIT) / 350.0, 0.05, 1.0)
            eng = engine_torque_nm(rpm) * self.throttle * self.torque_scale * limit
            mot = motor_nm(speed_kmh) * self.throttle * self.torque_scale if self.soc > 12 else 0.0
            drive = min(MU * MASS * G, (eng + mot) * GEARS[self.gear] * DRIVE_EFF / WHEEL_RADIUS)
        self.torque = eng + mot
        air = self.speed_mps + (headwind_mps(self.t_s) if self.speed_mps > 1.0 else 0.0)
        drag = CDA * air * abs(air)
        roll = CRR * MASS * G if self.speed_mps > 0.15 else 0.0
        grade = MASS * G * road_slope(self.t_s) if self.t_s >= 8.0 and self.gear > 0 else 0.0
        # 80 bar is a hard stop, a bit over 1 g before aero.
        brake_force = (self.brake_bar / 80.0) * 18500.0
        net = drive - drag - roll - grade - brake_force
        self.speed_mps = max(0.0, self.speed_mps + (net / MASS) * DT_S)

    def _rpm_target(self) -> float:
        if self.t_s < 1.1:
            return 0.0
        if self.t_s < 2.4:
            return (self.t_s - 1.1) / 1.3 * IDLE
        kin = kinematic_rpm(self.speed_mps, self.gear) if self.gear else 0.0
        if self.speed_mps < 0.4 or self.gear == 0:
            return IDLE + 160.0 * self.throttle
        requested = IDLE + 1900.0 * self.throttle
        if self.gear <= 2 and kin < requested and self.throttle > 0.2 and self.brake_bar < 1.0:
            return requested
        return max(kin, 800.0)

    def _track_rpm(self) -> None:
        kin = kinematic_rpm(self.speed_mps, self.gear) if self.gear else self._rpm_target()
        if self.shift_kind == "up":
            u = clamp(self.shift_elapsed / 0.11, 0.0, 1.0)
            blend = smoothstep(u)
            dip = -220.0 * math.sin(math.pi * u)
            self.rpm = self.shift_from_rpm + (kin - self.shift_from_rpm) * blend + dip
            return
        if self.shift_kind == "down":
            u = clamp(self.shift_elapsed / 0.16, 0.0, 1.0)
            blip = kin + 520.0 * math.sin(math.pi * u)
            blend = smoothstep(u)
            self.rpm = self.shift_from_rpm + (blip - self.shift_from_rpm) * blend
            return
        target = self._rpm_target()
        self.rpm = lag(self.rpm, target, 0.045)

    def _chassis(self, leg: tuple[float, float, float, float, float]) -> None:
        speed_kmh = self.speed_mps * 3.6
        self.lat_g = lag(self.lat_g, lat_target(self.t_s, leg), 0.22)
        self.lat_g = clamp(self.lat_g, -1.45, 1.45)
        if abs(self.lat_g) < 0.02 or self.speed_mps < 1.0:
            steer_target = 0.0
        else:
            radius = (self.speed_mps * self.speed_mps) / (self.lat_g * G)
            road = math.atan(WHEELBASE / radius)
            steer_target = math.degrees(road) * 14.0
        steer_target = clamp(steer_target, -420.0, 420.0)
        self.steer = lag(self.steer, steer_target, 0.16)
        self.esc_active = abs(self.lat_g) > 0.98 and speed_kmh > 55
        self.abs_active = self.brake_bar >= 42.0 and speed_kmh > 48.0

    def _thermal(self) -> None:
        self.load_avg = lag(self.load_avg, self.throttle, 4.0)
        self.coolant = lag(self.coolant, 84.0 + 12.0 * self.load_avg, 70.0)
        self.oil = lag(self.oil, 96.0 + 22.0 * self.load_avg, 130.0)
        self.oil_pressure = 1.15 + max(self.rpm, 0.0) / 3400.0
        self.fuel = max(41.0, self.fuel - self.throttle * 0.0045 * DT_S)
        amps = self.throttle * 150.0 * self.torque_scale - min(self.brake_bar, 36.0) * 1.4
        self.pack_a = lag(self.pack_a, amps, 0.35)
        self.soc = clamp(self.soc - self.pack_v * self.pack_a * DT_S / 3600.0 / 32000.0 * 100.0, 15.0, 100.0)
        self.pack_v = 412.0 - self.pack_a * 0.035 - (74.0 - self.soc) * 0.18
        self.cell_temp = lag(self.cell_temp, 27.0 + 14.0 * self.load_avg, 90.0)

    def doors_open(self) -> bool:
        return self.t_s < 3.2 or 548.0 < self.t_s < 572.0

    def turn_left(self) -> bool:
        t = self.t_s
        return 18 <= t < 30 or 210 <= t < 224 or 300 <= t < 314

    def turn_right(self) -> bool:
        t = self.t_s
        return 46 <= t < 64 or 224 <= t < 238 or 336 <= t < 356

    def wheel_kmh(self, index: int) -> float:
        """FL, FR, RL, RR. Outside wheels run faster in a corner."""
        base = self.speed_mps * 3.6
        side = -1.0 if index in (0, 2) else 1.0
        rear = index >= 2
        if abs(self.lat_g) < 0.03 or self.speed_mps < 1.0:
            ratio = 1.0
        else:
            radius = (self.speed_mps * self.speed_mps) / (self.lat_g * G)
            sign = 1.0 if radius > 0 else -1.0
            wheel_r = abs(radius) + side * sign * (TRACK / 2.0)
            ratio = wheel_r / abs(radius)
        slip = 1.0
        if rear and self.throttle > 0.45 and self.brake_bar < 1.0:
            slip += 0.006 * self.throttle
        if self.abs_active:
            slip *= 1.0 - 0.012 * (0.65 + 0.35 * math.sin(self.t_s * 55.0 + index))
        noise = 0.03 * math.sin(self.t_s * 9.0 + index * 1.7)
        return max(0.0, base * ratio * slip + noise)

    def payload(self, msg_id: int) -> bytes:
        counter = self.counters[msg_id]
        bad = False
        if msg_id == TCU and not self.counter_fault_used and self.t_s >= COUNTER_FAULT_S:
            counter = (counter + 3) & 0x0F
            self.counters[msg_id] = counter
            self.counter_fault_used = True
        if msg_id == ABS and not self.checksum_fault_used and self.t_s >= CHECKSUM_FAULT_S:
            bad = True
            self.checksum_fault_used = True
        data = bytearray(8)
        speed = self.speed_mps * 3.6
        if msg_id == ECM_FAST:
            shown_rpm = self.rpm + 3.0 * math.sin(self.t_s * 13.0)
            set_bits(data, 0, 16, round(clamp(shown_rpm, 0, 16000) / 0.25))
            set_signed(data, 16, 16, round((clamp(self.torque, -400, 1200) + 500) / 0.1))
            set_bits(data, 32, 8, round(self.throttle * 200))
            set_bits(data, 40, 8, round(min(1.0, self.throttle * 1.05) * 200))
            set_bits(data, 48, 4, self.gear)
            return finish(data, counter, 52, 4, bad)
        if msg_id == ECM_SLOW:
            set_bits(data, 0, 8, round(self.coolant + 40))
            set_bits(data, 8, 8, round(self.oil + 40))
            set_bits(data, 16, 16, round(self.oil_pressure / 0.01))
            set_bits(data, 32, 8, round(self.fuel / 0.5))
            set_bits(data, 40, 1, self.mil)
            set_bits(data, 41, 7, self.dtc_count)
            return finish(data, counter, 48, 8, bad)
        if msg_id == TCU:
            set_bits(data, 0, 4, self.gear)
            target = self.gear
            set_bits(data, 4, 4, target)
            set_bits(data, 8, 16, round(clamp(self.rpm, 0, 16000) / 0.25))
            clutch = 100 if self.shift_kind else 0
            set_bits(data, 24, 8, round(clutch / 0.5))
            set_bits(data, 32, 1, 1 if clutch else 0)
            return finish(data, counter, 48, 4, bad)
        if msg_id == ABS:
            for index, name_bit in enumerate((0, 12, 24, 36)):
                set_bits(data, name_bit, 12, round(self.wheel_kmh(index) / 0.1))
            return finish(data, counter, 48, 8, bad)
        if msg_id == ESC:
            set_signed(data, 0, 16, round(self.steer / 0.1))
            set_signed(data, 16, 16, round(self.lat_g / 0.001))
            set_bits(data, 32, 8, round(clamp(self.brake_bar, 0.0, 80.0) / 0.5))
            yaw = clamp(self.lat_g * 18, -40, 40)
            set_signed(data, 40, 12, round(yaw / 0.1))
            set_bits(data, 52, 1, 1 if self.abs_active else 0)
            set_bits(data, 53, 1, 1 if self.esc_active else 0)
            return finish(data, counter, 54, 2, bad)
        if msg_id == BCM:
            if self.doors_open():
                set_bits(data, 0, 1, 1)
            set_bits(data, 8, 1, 1 if self.turn_left() and int(self.t_s * 2) % 2 == 0 else 0)
            set_bits(data, 9, 1, 1 if self.turn_right() and int(self.t_s * 2) % 2 == 0 else 0)
            set_bits(data, 10, 1, 1 if self.brake_bar > 1.0 else 0)
            return finish(data, counter, 48, 8, bad)
        if msg_id == CLUSTER:
            shown = speed + 0.05 * math.sin(self.t_s * 2.1)
            shown_rpm = self.rpm + 3.0 * math.sin(self.t_s * 13.0 + 0.4)
            set_bits(data, 0, 16, round(max(0.0, shown) / 0.01))
            set_bits(data, 16, 16, round(clamp(shown_rpm, 0, 16000) / 0.25))
            telltale = 0
            if self.mil:
                telltale |= 1
            if self.abs_active:
                telltale |= 2
            if self.coolant > 105:
                telltale |= 4
            if self.turn_left():
                telltale |= 8
            if self.turn_right():
                telltale |= 16
            set_bits(data, 32, 8, telltale)
            return finish(data, counter, 48, 8, bad)
        if msg_id == BMS:
            set_bits(data, 0, 16, round(self.soc / 0.01))
            set_bits(data, 16, 16, round(self.pack_v / 0.1))
            set_signed(data, 32, 16, round(self.pack_a / 0.1))
            set_bits(data, 48, 8, round(self.cell_temp + 40))
            return finish(data, 0, 56, 0, bad)
        if msg_id == DIAG:
            code = 0x0301 if self.t_s >= DTC_S else 0
            set_bits(data, 0, 16, code)
            set_bits(data, 16, 8, 0x27 if code else 0)
            set_bits(data, 24, 8, 1 if code else 0)
            return finish(data, counter, 48, 8, bad)
        raise KeyError(msg_id)


def dbc_text() -> str:
    return '''VERSION "synthetic-hypercar"

NS_ :
BS_:
BU_: ECM TCU ABS ESC BCM IC BMS DIAG

BO_ 256 ECM_Fast: 8 ECM
 SG_ EngineRPM : 0|16@1+ (0.25,0) [0|16383.75] "rpm" TCU,IC
 SG_ EngTorque : 16|16@1- (0.1,-500) [-500|6053.5] "Nm" TCU
 SG_ Throttle : 32|8@1+ (0.5,0) [0|100] "%" IC
 SG_ AccelPedal : 40|8@1+ (0.5,0) [0|100] "%" TCU
 SG_ Gear : 48|4@1+ (1,0) [0|7] "" IC
 SG_ FastCounter : 52|4@1+ (1,0) [0|15] "" DIAG
 SG_ FastChecksum : 56|8@1+ (1,0) [0|255] "" DIAG

BO_ 257 ECM_Slow: 8 ECM
 SG_ CoolantTemp : 0|8@1+ (1,-40) [-40|215] "degC" IC
 SG_ OilTemp : 8|8@1+ (1,-40) [-40|215] "degC" IC
 SG_ OilPressure : 16|16@1+ (0.01,0) [0|10] "bar" IC
 SG_ FuelLevel : 32|8@1+ (0.5,0) [0|100] "%" IC
 SG_ MilLamp : 40|1@1+ (1,0) [0|1] "" IC
 SG_ DtcCount : 41|7@1+ (1,0) [0|127] "" DIAG
 SG_ SlowCounter : 48|8@1+ (1,0) [0|255] "" DIAG
 SG_ SlowChecksum : 56|8@1+ (1,0) [0|255] "" DIAG

BO_ 512 TCU_Gear: 8 TCU
 SG_ GearActual : 0|4@1+ (1,0) [0|15] "" ECM
 SG_ GearTarget : 4|4@1+ (1,0) [0|15] "" ECM
 SG_ InputShaftRPM : 8|16@1+ (0.25,0) [0|16383] "rpm" ECM
 SG_ ClutchPct : 24|8@1+ (0.5,0) [0|100] "%" ECM
 SG_ ShiftActive : 32|1@1+ (1,0) [0|1] "" ECM
 SG_ TcuCounter : 48|4@1+ (1,0) [0|15] "" DIAG
 SG_ TcuChecksum : 56|8@1+ (1,0) [0|255] "" DIAG

BO_ 768 ABS_Wheels: 8 ABS
 SG_ WheelFL : 0|12@1+ (0.1,0) [0|409.5] "km/h" ESC
 SG_ WheelFR : 12|12@1+ (0.1,0) [0|409.5] "km/h" ESC
 SG_ WheelRL : 24|12@1+ (0.1,0) [0|409.5] "km/h" ESC
 SG_ WheelRR : 36|12@1+ (0.1,0) [0|409.5] "km/h" ESC
 SG_ AbsCounter : 48|8@1+ (1,0) [0|255] "" DIAG
 SG_ AbsChecksum : 56|8@1+ (1,0) [0|255] "" DIAG

BO_ 784 ESC_Dynamics: 8 ESC
 SG_ SteeringAngle : 0|16@1- (0.1,0) [-1440|1440] "deg" IC
 SG_ LatG : 16|16@1- (0.001,0) [-32|32] "g" IC
 SG_ BrakePressure : 32|8@1+ (0.5,0) [0|80] "bar" ECM
 SG_ YawRate : 40|12@1- (0.1,0) [-204|204] "deg/s" IC
 SG_ AbsActive : 52|1@1+ (1,0) [0|1] "" IC
 SG_ EscActive : 53|1@1+ (1,0) [0|1] "" IC
 SG_ EscCounter : 54|2@1+ (1,0) [0|3] "" DIAG
 SG_ EscChecksum : 56|8@1+ (1,0) [0|255] "" DIAG

BO_ 1024 BCM_Body: 8 BCM
 SG_ DoorFL : 0|1@1+ (1,0) [0|1] "" IC
 SG_ TurnLeft : 8|1@1+ (1,0) [0|1] "" IC
 SG_ TurnRight : 9|1@1+ (1,0) [0|1] "" IC
 SG_ BrakeLamp : 10|1@1+ (1,0) [0|1] "" IC
 SG_ BcmCounter : 48|8@1+ (1,0) [0|255] "" DIAG
 SG_ BcmChecksum : 56|8@1+ (1,0) [0|255] "" DIAG

BO_ 1280 IC_Cluster: 8 IC
 SG_ VehicleSpeed : 0|16@1+ (0.01,0) [0|655] "km/h" ECM
 SG_ DisplayedRPM : 16|16@1+ (0.25,0) [0|16383] "rpm" ECM
 SG_ TelltaleMil : 32|1@1+ (1,0) [0|1] "" ECM
 SG_ TelltaleAbs : 33|1@1+ (1,0) [0|1] "" ECM
 SG_ TelltaleTemp : 34|1@1+ (1,0) [0|1] "" ECM
 SG_ TelltaleLeft : 35|1@1+ (1,0) [0|1] "" ECM
 SG_ TelltaleRight : 36|1@1+ (1,0) [0|1] "" ECM
 SG_ IcCounter : 48|8@1+ (1,0) [0|255] "" DIAG
 SG_ IcChecksum : 56|8@1+ (1,0) [0|255] "" DIAG

BO_ 1536 BMS_Pack: 8 BMS
 SG_ Soc : 0|16@1+ (0.01,0) [0|100] "%" IC
 SG_ PackVoltage : 16|16@1+ (0.1,0) [0|800] "V" IC
 SG_ PackCurrent : 32|16@1- (0.1,0) [-500|500] "A" IC
 SG_ CellTemp : 48|8@1+ (1,-40) [-40|120] "degC" IC

BO_ 1792 DIAG_Status: 8 DIAG
 SG_ DtcCode : 0|16@1+ (1,0) [0|65535] "" IC
 SG_ DtcStatus : 16|8@1+ (1,0) [0|255] "" IC
 SG_ DtcOccurrence : 24|8@1+ (1,0) [0|255] "" IC
 SG_ DiagCounter : 48|8@1+ (1,0) [0|255] "" ECM
 SG_ DiagChecksum : 56|8@1+ (1,0) [0|255] "" ECM

CM_ BO_ 256 "Synthetic ECM fast cycle. Not an OEM database.";
CM_ BO_ 768 "Wheel speeds. One frame near 460 s has a bad XOR checksum.";
CM_ BO_ 512 "Gearbox. One counter step is skipped near 200 s.";
BA_DEF_ BO_ "GenMsgCycleTime" INT 0 3600000;
BA_ "GenMsgCycleTime" BO_ 256 10;
BA_ "GenMsgCycleTime" BO_ 257 100;
BA_ "GenMsgCycleTime" BO_ 512 10;
BA_ "GenMsgCycleTime" BO_ 768 10;
BA_ "GenMsgCycleTime" BO_ 784 20;
BA_ "GenMsgCycleTime" BO_ 1024 100;
BA_ "GenMsgCycleTime" BO_ 1280 100;
BA_ "GenMsgCycleTime" BO_ 1536 100;
BA_ "GenMsgCycleTime" BO_ 1792 1000;
'''


def generate() -> tuple[int, int]:
    bus = Bus()
    due = {msg_id: period // 5 for msg_id, period, _node in MESSAGES}
    periods = {msg_id: period for msg_id, period, _node in MESSAGES}
    lines: list[tuple[int, int, str]] = []
    events: list[tuple[int, str]] = [
        (0, "Key on"),
        (1_200_000, "Crank"),
        (3_200_000, "Driver door closed"),
        (8_000_000, "Pullaway"),
        (int(DTC_S * 1_000_000), "DTC P0301 misfire"),
        (int(COUNTER_FAULT_S * 1_000_000), "TCU counter skip"),
        (int(ECM_GAP[0] * 1_000_000), "ECM_Fast missing"),
        (int(BUS_OFF[0] * 1_000_000), "Bus-off"),
        (int(CHECKSUM_FAULT_S * 1_000_000), "ABS checksum"),
        (548_000_000, "Pit, driver door open"),
        (600_000_000, "Key off"),
    ]
    end_us = int(DURATION_S * 1_000_000)
    last_gear = 0
    abs_seen = False
    order = 0
    steps = int(DURATION_S / DT_S)
    for _ in range(steps + 1):
        t_us = int(round(bus.t_s * 1_000_000))
        if bus.gear != last_gear:
            if bus.gear > last_gear and bus.gear >= 1:
                events.append((t_us, f"Upshift {bus.gear}"))
            elif last_gear > bus.gear and last_gear > 0:
                events.append((t_us, f"Downshift {bus.gear}"))
            last_gear = bus.gear
        if bus.abs_active and not abs_seen:
            events.append((t_us, "ABS"))
            abs_seen = True
        elif not bus.abs_active:
            abs_seen = False
        in_bus_off = BUS_OFF[0] <= bus.t_s < BUS_OFF[1]
        in_gap = ECM_GAP[0] <= bus.t_s < ECM_GAP[1]
        for msg_id, _period, _node in MESSAGES:
            if due[msg_id] > t_us:
                continue
            stamp = due[msg_id]
            if in_bus_off:
                due[msg_id] = stamp + periods[msg_id]
                continue
            if in_gap and msg_id == ECM_FAST:
                due[msg_id] = stamp + periods[msg_id]
                continue
            payload = bus.payload(msg_id)
            lines.append((stamp, order, f"F {stamp} {msg_id:03X} {payload.hex().upper()}"))
            order += 1
            bus.counters[msg_id] = (bus.counters[msg_id] + 1) & 0xFF
            jitter = bus.rng.randint(-periods[msg_id] // 12, periods[msg_id] // 12)
            nxt = stamp + periods[msg_id] + jitter
            due[msg_id] = max(nxt, stamp + (periods[msg_id] * 8) // 10)
        bus.step()

    burst = int(BUS_OFF[0] * 1_000_000)
    for index in range(12):
        stamp = burst + index * 2_000
        lines.append((stamp, 50, f"X {stamp}"))
    for stamp, label in events:
        lines.append((stamp, 40, f"E {stamp} {label}"))
    lines.sort()
    header = [
        "SLOGv1",
        "# SYNTHETIC. Not a vehicle capture. Generated by scripts/hypercar_bus.py.",
        "# Hybrid hypercar. Torque, gears, drag, grade, and wind. Urban, corners, a short top-speed straight, pit.",
        "# XOR checksum in byte 7. Counters live in the DBC. Pair with hypercar_lap.dbc.",
        "# Planted: DTC P0301, TCU counter skip, ECM_Fast gap, bus-off error frames, ABS bad checksum.",
    ]
    text = "\n".join(header + [line for _t, _order, line in lines]) + "\n"
    FIXTURES.mkdir(parents=True, exist_ok=True)
    (FIXTURES / "hypercar_lap.slog").write_text(text, encoding="utf-8", newline="\n")
    (FIXTURES / "hypercar_lap.dbc").write_text(dbc_text(), encoding="utf-8", newline="\n")
    return len(lines), len(text.encode("utf-8"))


if __name__ == "__main__":
    count, size = generate()
    print(f"hypercar_lap.slog records={count} bytes={size}")
