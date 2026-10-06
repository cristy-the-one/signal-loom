#!/usr/bin/env python3
"""Write the synthetic instrument-cluster log, signal map, and demo project.

The recording is invented. It is a 45 second key-on drive used as the
sample deck, not a capture from a vehicle.
"""

from __future__ import annotations

import json
import math
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "fixtures"
DURATION_S = 45.0


def speed_kmh(t_s: float) -> float:
    if t_s < 2:
        return 0.0
    if t_s < 12:
        return (t_s - 2) / 10 * 52
    if t_s < 30:
        return 52 + 1.2 * math.sin((t_s - 12) * 0.7)
    if t_s < 38:
        return max(0.0, 52 * (38 - t_s) / 8)
    return 0.0


def gear_of(speed: float) -> int:
    if speed < 1:
        return 0
    if speed < 18:
        return 1
    if speed < 35:
        return 2
    return 3


def throttle(t_s: float) -> float:
    if t_s < 1:
        return 0.0
    if t_s < 2:
        return 25.0
    if t_s < 12:
        return 68.0
    if t_s < 30:
        return 16.0
    return 0.0


def rpm(t_s: float, speed: float, gear: int, thr: float) -> float:
    if gear == 0:
        return 780 + thr * 3.2
    ratio = {1: 95, 2: 58, 3: 36}[gear]
    return 800 + speed * ratio + thr * 4


def coolant(t_s: float) -> float:
    return 68 + 20 * (t_s / DURATION_S)


def fuel(t_s: float) -> float:
    return 63.5 - 0.8 * (t_s / DURATION_S)


def battery(t_s: float) -> float:
    return 12.4 if t_s < 1 else 14.2


def turn(t_s: float) -> int:
    if 8 <= t_s < 14:
        return 1
    if 32 <= t_s < 36:
        return 2
    return 0


def brake_bar(t_s: float) -> float:
    if 30 <= t_s <= 38:
        return 32.0
    return 0.0


def set_le(data: bytearray, start: int, length: int, raw: int) -> None:
    raw = max(0, int(raw))
    for bit in range(length):
        if (raw >> bit) & 1:
            index = start + bit
            data[index // 8] |= 1 << (index % 8)


def pack_powertrain(t_s: float) -> bytes:
    speed = speed_kmh(t_s)
    gear = gear_of(speed)
    thr = throttle(t_s)
    data = bytearray(8)
    set_le(data, 0, 16, round(rpm(t_s, speed, gear, thr) / 0.25))
    set_le(data, 16, 16, round(speed / 0.01))
    set_le(data, 32, 8, round(coolant(t_s) + 40))
    set_le(data, 40, 8, round(thr / 0.4))
    return bytes(data)


def pack_body(t_s: float) -> bytes:
    data = bytearray(8)
    set_le(data, 0, 4, gear_of(speed_kmh(t_s)))
    set_le(data, 8, 12, round(brake_bar(t_s) / 0.1))
    set_le(data, 20, 2, turn(t_s))
    return bytes(data)


def pack_elec(t_s: float, odo_km: float) -> bytes:
    data = bytearray(8)
    set_le(data, 0, 8, round(fuel(t_s) / 0.5))
    set_le(data, 8, 8, round(battery(t_s) / 0.1))
    set_le(data, 16, 24, round(odo_km / 0.1))
    return bytes(data)


def hex_bytes(data: bytes) -> str:
    return data.hex().upper()


def main() -> None:
    FIXTURES.mkdir(parents=True, exist_ok=True)
    records: list[tuple[int, int, str]] = []
    odo = 128_430.2
    last_elec = 0.0
    last_gear = 0
    gear_events: list[tuple[int, str]] = []

    t_us = 0
    end_us = int(DURATION_S * 1_000_000)
    while t_us <= end_us:
        t_s = t_us / 1_000_000
        speed = speed_kmh(t_s)
        gear = gear_of(speed)
        if gear != last_gear and gear >= 2:
            name = {2: "Into 2nd", 3: "Into 3rd"}[gear]
            gear_events.append((t_us, name))
            last_gear = gear
        elif gear != last_gear:
            last_gear = gear
        if t_us % 20_000 == 0:
            records.append((t_us, 0, f"F {t_us} 1A0 {hex_bytes(pack_powertrain(t_s))}"))
        if t_us % 50_000 == 0:
            records.append((t_us, 1, f"F {t_us} 2B0 {hex_bytes(pack_body(t_s))}"))
        if t_us % 100_000 == 0:
            dt = t_s - last_elec
            odo += speed * dt / 3600
            last_elec = t_s
            records.append((t_us, 2, f"F {t_us} 3C0 {hex_bytes(pack_elec(t_s, odo))}"))
        t_us += 10_000

    scripted = [
        (0, "Key on"),
        (1_000_000, "Crank"),
        (2_000_000, "Pullaway"),
        (8_000_000, "Lane change"),
        (30_000_000, "Braking"),
        (40_000_000, "Stop"),
        (45_000_000, "Key off"),
    ]
    for t, label in scripted + gear_events:
        records.append((t, 3, f"E {t} {label}"))

    records.sort()
    lines = [
        "SLOGv1",
        "# Synthetic instrument-cluster drive. Not a recorded vehicle.",
        "# t_us is microseconds from key-on. Payloads are 8 bytes, little-endian.",
        "# Pair with cluster.map.json.",
    ]
    lines.extend(line for _t, _order, line in records)
    (FIXTURES / "cluster_drive.slog").write_text("\n".join(lines) + "\n", encoding="utf-8")

    signal_map = {
        "name": "Instrument cluster",
        "version": 1,
        "messages": [
            {
                "id": "0x1A0",
                "name": "Powertrain",
                "signals": [
                    {"name": "EngineRPM", "startBit": 0, "bitLength": 16, "factor": 0.25, "offset": 0, "unit": "rpm"},
                    {"name": "VehicleSpeed", "startBit": 16, "bitLength": 16, "factor": 0.01, "offset": 0, "unit": "km/h"},
                    {"name": "CoolantTemp", "startBit": 32, "bitLength": 8, "factor": 1, "offset": -40, "unit": "°C"},
                    {"name": "Throttle", "startBit": 40, "bitLength": 8, "factor": 0.4, "offset": 0, "unit": "%"},
                ],
            },
            {
                "id": "0x2B0",
                "name": "Body",
                "signals": [
                    {"name": "Gear", "startBit": 0, "bitLength": 4, "factor": 1, "offset": 0, "unit": ""},
                    {"name": "BrakePressure", "startBit": 8, "bitLength": 12, "factor": 0.1, "offset": 0, "unit": "bar"},
                    {"name": "TurnSignal", "startBit": 20, "bitLength": 2, "factor": 1, "offset": 0, "unit": ""},
                ],
            },
            {
                "id": "0x3C0",
                "name": "Electrical",
                "signals": [
                    {"name": "FuelLevel", "startBit": 0, "bitLength": 8, "factor": 0.5, "offset": 0, "unit": "%"},
                    {"name": "BatteryVoltage", "startBit": 8, "bitLength": 8, "factor": 0.1, "offset": 0, "unit": "V"},
                    {"name": "Odometer", "startBit": 16, "bitLength": 24, "factor": 0.1, "offset": 0, "unit": "km"},
                ],
            },
        ],
    }
    (FIXTURES / "cluster.map.json").write_text(json.dumps(signal_map, indent=2) + "\n", encoding="utf-8")

    project = {
        "format": "signal-loom",
        "version": 1,
        "logPath": "fixtures/cluster_drive.slog",
        "signalMapPath": "fixtures/cluster.map.json",
        "bookmarks": [
            {"id": "key-on", "tUs": 0, "label": "Key on"},
            {"id": "pullaway", "tUs": 2_000_000, "label": "Pullaway"},
            {"id": "braking", "tUs": 30_000_000, "label": "Braking"},
        ],
        "view": {
            "playheadUs": 2_000_000,
            "spanUs": 45_000_000,
            "plotted": ["VehicleSpeed", "EngineRPM", "BrakePressure"],
        },
    }
    (FIXTURES / "demo.loom").write_text(json.dumps(project, indent=2) + "\n", encoding="utf-8")

    snippet = "\n".join(
        [
            "t_us,signal,value,unit",
            "0,VehicleSpeed,0,km/h",
            "0,EngineRPM,800,rpm",
            "1000000,VehicleSpeed,12.5,km/h",
            "1000000,EngineRPM,1600,rpm",
            "2000000,VehicleSpeed,28,km/h",
            "2000000,EngineRPM,2400,rpm",
            "",
        ]
    )
    (FIXTURES / "decoded_snippet.csv").write_text(snippet, encoding="utf-8")
    print(f"wrote {len(records)} records, gear events: {gear_events}")


if __name__ == "__main__":
    main()
