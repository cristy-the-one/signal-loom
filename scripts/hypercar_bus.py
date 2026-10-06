#!/usr/bin/env python3
"""Synthetic 10-minute hybrid hypercar bus.

This is not a capture. Message ids, checksums, and the lap are invented
for Signal Loom. The layout is a private bus: ECM, TCU, ABS, ESC, BCM,
cluster, and BMS. Payloads use an XOR checksum in byte 7 and a nibble
or byte counter. Cycle times are 10, 20, 100, and 1000 ms, with jitter.
"""

from __future__ import annotations

import math
import random
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "fixtures"
DURATION_S = 600.0
DT_S = 0.001

# Overall ratio, engine rpm / wheel rpm. Tuned so 7th sits near 7,500 rpm
# at about 280 km/h with a 0.34 m tyre.
GEAR_RATIO = [0.0, 24.0, 15.5, 10.6, 7.6, 5.7, 4.6, 3.85]
WHEEL_RADIUS = 0.34
WHEEL_RPM_PER_KMH = 7.80
MASS = 1480.0
MU_FORCE = 1.25 * MASS * 9.81

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


def clamp(value: float, lo: float, hi: float) -> float:
    return max(lo, min(hi, value))


def targets(t_s: float) -> tuple[float, float]:
    """Target speed (km/h) and steering angle (deg) for the scripted lap."""
    segments = [
        (0, 8, 0, 0, 0, 0),
        (8, 28, 0, 38, 0, 0),
        (28, 42, 38, 0, 0, 0),
        (42, 68, 0, 52, 0, 6),
        (68, 86, 52, 8, 6, -10),
        (86, 150, 8, 160, -10, 0),
        (150, 296, 160, 248, 0, 0.3),
        (296, 302, 248, 25, 0.3, 1),
        (302, 370, 25, 32, 1, 8),
        (370, 410, 30, 150, 8, 16),
        (410, 445, 150, 55, 16, -24),
        (445, 490, 55, 210, -24, 6),
        (490, 525, 210, 40, 6, -12),
        (525, 555, 40, 8, -12, 0),
        (555, 600, 8, 0, 0, 0),
    ]
    for start, end, v0, v1, s0, s1 in segments:
        if start <= t_s < end or (end == 600 and t_s >= start):
            span = end - start
            blend = 0.0 if span == 0 else (t_s - start) / span
            return v0 + (v1 - v0) * blend, s0 + (s1 - s0) * blend
    return 0.0, 0.0


def torque_curve(rpm: float) -> float:
    if rpm < 900:
        return 0.25
    if rpm < 3200:
        return 0.45 + 0.55 * (rpm - 900) / 2300
    if rpm < 6800:
        return 1.0
    if rpm < 8600:
        return max(0.2, 1.0 - 0.85 * (rpm - 6800) / 1800)
    return 0.15


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
        self.brake = 0.0
        self.torque = 0.0
        self.rpm = 820.0
        self.steer = 0.0
        self.lat_g = 0.0
        self.coolant = 38.0
        self.oil = 24.0
        self.oil_pressure = 1.4
        self.fuel = 58.0
        self.soc = 71.0
        self.pack_v = 392.0
        self.pack_a = 0.0
        self.cell_temp = 26.0
        self.abs_active = False
        self.esc_active = False
        self.abs_until = -1.0
        self.mil = 0
        self.dtc_count = 0
        self.shift_at = -10.0
        self.counters = {msg[0]: 0 for msg in MESSAGES}
        self.counter_fault_used = False
        self.checksum_fault_used = False
        self.t_s = 0.0

    def step(self) -> None:
        target_v, target_steer = targets(self.t_s)
        speed_kmh = self.speed_mps * 3.6
        if self.t_s < 1.1:
            self.throttle = 0.0
            self.brake = 0.0
            self.rpm = 0.0
        elif self.t_s < 2.4:
            self.throttle = 0.18
            self.brake = 0.0
            self.rpm = 280 + (self.t_s - 1.1) / 1.3 * 700
        else:
            if target_v > speed_kmh + 1.2:
                self.throttle = clamp((target_v - speed_kmh) / 22.0, 0.08, 1.0)
                self.brake = 0.0
            elif speed_kmh > target_v + 1.2:
                self.throttle = 0.0
                self.brake = clamp((speed_kmh - target_v) / 28.0, 0.0, 1.0)
            else:
                self.throttle = 0.1 if target_v > 6 else 0.0
                self.brake = 0.0
            self._shift(speed_kmh)
            wheel_rpm = speed_kmh * WHEEL_RPM_PER_KMH
            if self.gear == 0:
                self.rpm = 830 + self.throttle * 900 + 6 * math.sin(self.t_s * 23)
            else:
                self.rpm = max(750.0, wheel_rpm * GEAR_RATIO[self.gear])
                self.rpm += 8 * math.sin(self.t_s * 19)
            if self.t_s - self.shift_at < 0.12 and self.gear:
                # Clutch fill: the drop is already in the new ratio. Add a short flare.
                self.rpm += 280 * (1 - (self.t_s - self.shift_at) / 0.12)

        engine = self.throttle * 820.0 * torque_curve(self.rpm if self.rpm else 800)
        motor = self.throttle * 240.0 if self.soc > 12 else 0.0
        regen = self.brake * 160.0 if speed_kmh > 8 else 0.0
        self.torque = engine + motor - regen
        if self.gear == 0 or self.t_s < 2.4:
            drive = 0.0
        else:
            drive = min(MU_FORCE, self.torque * GEAR_RATIO[self.gear] / WHEEL_RADIUS)
        drag = 0.36 * self.speed_mps * self.speed_mps
        roll = 160.0 if self.speed_mps > 0.2 else 0.0
        brake_force = self.brake * 16500.0
        net = drive - drag - roll - brake_force
        self.speed_mps = max(0.0, self.speed_mps + (net / MASS) * DT_S)

        self.steer += (target_steer - self.steer) * 0.04
        speed = max(self.speed_mps, 0.0)
        lat = (self.steer / 16.0) * min(speed_kmh / 70.0, 1.35)
        self.lat_g = clamp(lat, -1.45, 1.45)
        self.esc_active = abs(self.lat_g) > 1.05 and speed_kmh > 40

        if self.brake > 0.72 and speed_kmh > 36:
            self.abs_active = True
            self.abs_until = self.t_s + 0.28
        elif self.t_s > self.abs_until:
            self.abs_active = False

        self.coolant += ((92.0 - self.coolant) * 0.018 + self.throttle * 0.55) * DT_S
        self.oil += ((112.0 - self.oil) * 0.006 + self.throttle * 0.22) * DT_S
        self.oil_pressure = 1.15 + max(self.rpm, 0.0) / 3200.0
        self.fuel = max(40.0, self.fuel - self.throttle * 0.012 * DT_S)
        self.soc = clamp(
            self.soc - self.throttle * 0.045 * DT_S + self.brake * 0.02 * DT_S,
            18.0,
            100.0,
        )
        self.pack_a = self.throttle * 280.0 - (regen if self.brake > 0.05 else 0.0)
        self.pack_v = 398.0 - self.pack_a * 0.045 - (71.0 - self.soc) * 0.2
        self.cell_temp += (28.0 + self.throttle * 16.0 - self.cell_temp) * 0.025 * DT_S
        if self.t_s >= DTC_S:
            self.mil = 1
            self.dtc_count = 1
        self.t_s += DT_S

    def _shift(self, speed_kmh: float) -> None:
        if self.t_s - self.shift_at < 0.55:
            return
        if self.gear == 0:
            if self.throttle > 0.15 or speed_kmh > 2:
                self.gear = 1
                self.shift_at = self.t_s
            return
        if self.rpm > 7100 and self.gear < 7:
            self.gear += 1
            self.shift_at = self.t_s
        elif self.rpm < 2500 and self.gear > 1 and self.throttle < 0.45:
            self.gear -= 1
            self.shift_at = self.t_s

    def doors_open(self) -> bool:
        return self.t_s < 3.2 or 548.0 < self.t_s < 572.0

    def turn_left(self) -> bool:
        t = self.t_s
        return 18 <= t < 27 or 398 <= t < 418 or 500 <= t < 514

    def turn_right(self) -> bool:
        t = self.t_s
        return 62 <= t < 80 or 428 <= t < 450

    def wheel_kmh(self, index: int) -> float:
        base = self.speed_mps * 3.6
        steer = self.steer / 350.0
        side = -1 if index in (0, 2) else 1
        axle = 0.004 * side * steer * (1 if index < 2 else 0.6)
        slip = 1.0 + self.throttle * 0.008 if index < 2 else 1.0
        pulse = 0.0
        if self.abs_active:
            pulse = 0.035 * math.sin(self.t_s * 90 + index * 1.3)
        return max(0.0, base * slip * (1 + axle + pulse))

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
            set_bits(data, 0, 16, round(clamp(self.rpm, 0, 16000) / 0.25))
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
            clutch = 100 if self.t_s - self.shift_at < 0.12 else 0
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
            set_bits(data, 32, 8, round(self.brake * 160))
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
            set_bits(data, 10, 1, 1 if self.brake > 0.05 else 0)
            return finish(data, counter, 48, 8, bad)
        if msg_id == CLUSTER:
            set_bits(data, 0, 16, round(speed / 0.01))
            set_bits(data, 16, 16, round(clamp(self.rpm, 0, 16000) / 0.25))
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
        "# Hybrid hypercar, urban then highway then a lap. 10 minutes from key-on.",
        "# XOR checksum in byte 7. Counters live in the DBC. Pair with hypercar_lap.dbc.",
        "# Planted: DTC P0301, TCU counter skip, ECM_Fast gap, bus-off error frames, ABS bad checksum.",
    ]
    text = "\n".join(header + [line for _t, _order, line in lines]) + "\n"
    FIXTURES.mkdir(parents=True, exist_ok=True)
    (FIXTURES / "hypercar_lap.slog").write_text(text, encoding="utf-8")
    (FIXTURES / "hypercar_lap.dbc").write_text(dbc_text(), encoding="utf-8")
    return len(lines), len(text.encode("utf-8"))


if __name__ == "__main__":
    count, size = generate()
    print(f"hypercar_lap.slog records={count} bytes={size}")
