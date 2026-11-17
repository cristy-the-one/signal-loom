#!/usr/bin/env python3
"""Write the synthetic §4.4 brake contract fixture.

The recording is Vector ASC because loom-core already reads that format
(`crates/loom-core/src/scan/asc.rs`) and §5 names the file `brake.asc`.
Times are absolute seconds with six decimals.

`expected-verdict.json` is hand-written from §4.4 and §5. This script does
not compute a verdict, and it does not write that file.
"""

from __future__ import annotations

import hashlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "fixtures" / "judge"

END_US = 10_000_000
STEP_US = 10_000
# No Lamps frame from 4990 ms through 5200 ms. The last one before that is 4980 ms.
GAP_LO_US = 4_990_000
GAP_HI_US = 5_200_000

PEDAL_EDGES = (
    (0, 0),
    (1_000_000, 1),
    (2_000_000, 0),
    (3_000_000, 1),
    (4_000_000, 0),
    (5_000_000, 1),
    (6_000_000, 0),
    (9_990_000, 1),
)

REQUEST = """\
{
  "format": "loom-judge/1",
  "spec_id": "default@REQ-BRK-001",
  "cases": [
    {
      "case_id": "TC-1",
      "verifies": ["REQ-BRK-001"],
      "kind": "positive",
      "checks": [
        {
          "id": "c1",
          "when": {
            "signal": {"message": "BrakeCmd", "signal": "BrakePedal"},
            "op": "==",
            "value": 1
          },
          "expect": {
            "signal": {"message": "Lamps", "signal": "BrakeLamp"},
            "op": "==",
            "value": 1
          },
          "mode": "eventually",
          "within_ms": [0, 100]
        }
      ]
    }
  ]
}
"""

DBC = """\
VERSION ""

NS_ :
BS_:

BU_: ESC BCM

BO_ 256 BrakeCmd: 8 ESC
 SG_ BrakePedal : 0|8@1+ (1,0) [0|1] "" ESC

BO_ 512 Lamps: 8 BCM
 SG_ BrakeLamp : 0|1@1+ (1,0) [0|1] "" BCM

BA_DEF_ BO_ "GenMsgCycleTime" INT 0 3600000;
BA_ "GenMsgCycleTime" BO_ 256 10;
BA_ "GenMsgCycleTime" BO_ 512 10;
"""

# Base names, in the order SHA256SUMS lists them. expected-verdict.json is
# not produced here; it is included once it has been written by hand.
SUMMED = ("brake.asc", "brake.dbc", "expected-verdict.json", "request.json")


def pedal(t_us: int) -> int:
    value = 0
    for at, nxt in PEDAL_EDGES:
        if t_us >= at:
            value = nxt
    return value


def lamp(t_us: int) -> int:
    if t_us >= 2_030_000:
        return 0
    if t_us >= 1_040_000:
        return 1
    return 0


def stamp(t_us: int) -> str:
    sec, usec = divmod(t_us, 1_000_000)
    return f"{sec}.{usec:06d}"


def payload(value: int) -> str:
    return " ".join([f"{value:02X}"] + ["00"] * 7)


def frame_line(t_us: int, can_id: str, value: int) -> str:
    # Channel 1 is the first bus, matching the ASC samples loom-core already reads.
    return f"{stamp(t_us)} 1 {can_id} Rx d 8 {payload(value)}"


def recording() -> tuple[str, int, int, int]:
    lines = [
        "date Mon Nov 17 00:00:00.000 2026",
        "base hex timestamps absolute",
        "internal events logged",
        "// Synthetic brake contract. Not a recorded vehicle.",
    ]
    brake = 0
    lamps = 0
    t_us = 0
    while t_us <= END_US:
        lines.append(frame_line(t_us, "100", pedal(t_us)))
        brake += 1
        if not GAP_LO_US <= t_us <= GAP_HI_US:
            lines.append(frame_line(t_us, "200", lamp(t_us)))
            lamps += 1
        t_us += STEP_US
    return "\n".join(lines) + "\n", brake + lamps, brake, lamps


def write_text(path: Path, text: str) -> None:
    path.write_text(text, encoding="utf-8", newline="\n")


def write_sums() -> None:
    lines = []
    for name in SUMMED:
        path = OUT / name
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        lines.append(f"{digest}  {name}")
    write_text(OUT / "SHA256SUMS", "\n".join(lines) + "\n")


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    text, frames, brake, lamps = recording()
    write_text(OUT / "brake.asc", text)
    write_text(OUT / "brake.dbc", DBC)
    write_text(OUT / "request.json", REQUEST)
    print(f"brake.asc frames={frames} brake={brake} lamps={lamps}")
    expected = OUT / "expected-verdict.json"
    if not expected.is_file():
        raise SystemExit(
            "expected-verdict.json is hand-written from plan §4.4 and §5; "
            "this script does not compute a verdict"
        )
    write_sums()
    print(f"wrote {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
