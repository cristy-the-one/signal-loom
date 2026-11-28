# Judging generated tests from recordings — plan and contract

**Status:** draft for the owner, 2026-11-17. The decisions in §9 have defaults so work can start;
the owner may overturn any of them before the cards that depend on it are merged.
**Written for:** implementers of any capability. Cards marked **needs a capable model** say why.

---

## 1. Why

grap-rust turns requirements into test specifications. Each test case says what must be observed
(`expected_observable_result`) and how to decide (`pass_criterion`). Today that is prose.

signal-loom already reads the recordings such a test is run on — ASC, BLF, candump, CSV, SLOG —
decodes them with the DBC, and knows when a message was late, a counter skipped or a checksum was
bad.

Joined, the two answer the question an engineer is actually asked: **did this requirement hold on
this recording?** — with the timestamps and values that show it, and with an honest
"could not tell" when the recording cannot show it.

```
requirement ──grap-rust──▶ test case + checks ──export──▶ judge request (loom-judge/1)
                                                                │
                              recording + DBC ──signal-loom─────┤
                                                                ▼
requirement ◀──grap-rust──── events + evidence ◀──import──── verdicts (loom-verdict/1)
```

The two tools never call each other. They exchange two JSON files whose format is fixed in this
document. That keeps each repository testable on its own and keeps recordings, which may be
customer data, on the machine where they already are.

## 2. Scope

**In:** checks on decoded signal values over time, relative to an event in the same recording.
One recording and one or more DBC files per request. A command-line judge with no window. The
evidence for every verdict.

**Not in this plan:** driving a test bench, sending frames, diagnostic requests (UDS/XCP),
time-synchronising two recordings, checks on raw bytes, video or analogue channels. Each of those
is a later decision, not a gap to fill quietly.

**Never:** a recording, a DBC or a verdict file is not sent anywhere by either tool. signal-loom
does not transmit (README); the judge adds no network code.

---

## 3. The judge request — `loom-judge/1`

One JSON object. Every key below is required unless marked optional. Unknown keys are refused.

An optional key may also be present as `null` (or, for `given`, as `[]`); that means the same as
leaving it out. This lets a producer whose model server requires every schema key — as strict
JSON-schema modes do — always write every key.

```json
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
          "given": [
            {"signal": {"message": "Ignition", "signal": "IgnState"}, "op": "==", "value": "ON"}
          ],
          "when":   {"signal": {"message": "BrakeCmd", "signal": "BrakePedal"}, "op": "==", "value": 1},
          "expect": {"signal": {"message": "Lamps", "signal": "BrakeLamp"}, "op": "==", "value": 1},
          "mode": "eventually",
          "within_ms": [0, 100]
        }
      ]
    }
  ]
}
```

### 3.1 A condition

`{"signal": {"message": M, "signal": S, "channel": C}, "op": OP, "value": V, "tolerance": T}`

- `message` and `signal` are the **names in the DBC**: the `BO_` name and the `SG_` name. Not a
  CAN id, not an ARXML path, not signal-loom's display name `Name@<id>`.
- `channel` (optional, integer) picks one channel when the same DBC is loaded on several. Without
  it, a reference that matches more than one loaded signal is refused as ambiguous, naming the
  channels it matched. A `channel` that matches nothing is refused like an unknown signal.
- `op` is one of `==`, `!=`, `>`, `>=`, `<`, `<=`.
- `value` is a number (the physical value, after factor and offset), or a string. A string must be
  a `VAL_` label of that signal; it stands for the physical value that label has.
- `tolerance` (optional, number ≥ 0) applies to `==` and `!=` only.
  `==` holds when `|x − value| ≤ tolerance`. Default: half the signal's factor when the DBC gives
  one, else 0. A `tolerance` on any other op is refused.

### 3.2 A check

| Key | Meaning |
|---|---|
| `id` | unique within the case |
| `given` | optional list of conditions; all must hold **at the anchor instant** for that anchor to count |
| `when` | the condition whose **rising edge** is the anchor (see §4.1) |
| `expect` | the condition judged in the window |
| `mode` | `eventually` (holds at least once in the window), `always` (holds throughout), `never` (holds at no point) |
| `within_ms` | `[min, max]`, integers, `0 ≤ min ≤ max ≤ 3 600 000`; the window is `[anchor + min, anchor + max]` |

A case may have zero checks. Such a case is **not judgeable** and is reported as such; it is not an
error.

### 3.3 Refusal

The request is checked completely before any recording is read, and refused — with **every**
problem listed — if: `format` is not `loom-judge/1`; a key is unknown or missing; two cases share a
`case_id`, or two checks in a case share an `id`; an `op`, `mode` or `kind` is not in its list;
`within_ms` breaks its rule; a `tolerance` is negative or on the wrong op; a signal is not in the
loaded DBC files, or matches more than one loaded signal and names no `channel`; a string `value` is not a label of that signal.

A refusal is exit code 2 and writes no verdict file.

---

## 4. Evaluation

All of this runs on **every sample** of the signals involved. The windowed `query` path decimates
to a point budget and must not be used here.

### 4.1 Held values and the anchor

- A signal's value at time `t` is its last decoded sample at or before `t`. Before its first sample
  it has **no value**. A condition on a signal with no value is **unknown**, not false.
- The `when` condition is evaluated at every sample of its signal. An **anchor** is a sample where
  it is true and the previous evaluation was false. The first sample of the signal is never an
  anchor, because there is no earlier state to rise from.
- At each anchor, `given` is evaluated with held values. If any condition is false, the anchor is
  **skipped** and counted. If any is unknown, the anchor is judged **inconclusive** with reason
  `given_unknown`.

### 4.2 One occurrence

For an anchor at `a`, the window is `[a + min, a + max]`. The `expect` condition is evaluated at
the window start (held value) and at every sample of its signal inside the window.

| Mode | pass | fail |
|---|---|---|
| `eventually` | some evaluation is true | the whole window is seen and none is true |
| `always` | the whole window is seen and every evaluation is true | some evaluation is false |
| `never` | the whole window is seen and none is true | some evaluation is true |

**A decision that is already certain is made; otherwise the occurrence is inconclusive.** It is
inconclusive, with the first reason that applies, when none of the above is reached and:

1. `window_past_end` — the window ends after the recording ends;
2. `no_value` — `expect` is unknown at the window start and, for `eventually`, no sample arrives
   in the window. For `always` and `never` an unknown start is enough (see the points below): the
   table's "whole window is seen" then never holds;
3. `integrity` — inside the window, before the decision, either the `expect` message goes silent
   for longer than its timeout, or the `expect` signal has a checksum or counter event.
   *Silent* uses the same cycle time and factor as signal-loom's timeout check, but is computed by
   the judge from the frames: the moment of the timeout is the last frame plus that threshold. This
   does not depend on where the timeout event is drawn — which may be when the late frame finally
   arrives, after the window has closed.

Evidence for every occurrence: `anchor_us`; for a decision, the `t_us` and `value` of the sample
that decided it (for an `eventually` fail and for `always`/`never` passes, which no single sample
decides: the window end and the value held there); for an
inconclusive one, the reason and, for `integrity`, the event's `t_us` and kind.

An eventually pass whose expect already held just before the anchor is marked already_held; it stays a pass.

**Points §4.2 left open, fixed by SL-13 and covered by its tests:**

- The window start is evaluated on the value held there, reported at the window start. When
  samples sit exactly on the start, each of them is evaluated, as every sample inside the window
  is, and the value held from before is not: they replace it.
- Silence is a gap longer than the threshold, compared as the indexer's timeout check compares it
  (a gap exactly equal is not silence); its moment is the last frame plus the threshold, rounded up
  to a whole microsecond. A frame counts only on a channel the signal is decoded on, the rule
  `decode_frame` uses, so another channel's frames do not hide a silent one.
- Silence counts while it lasts. A message that went silent before the window opened and is still
  silent inside it is an `integrity` problem from the window start. The stretch after a message's
  last frame, up to the end of the recording, is silence too.
- `always` and `never` cannot pass from an unknown start: the unseen head of the window could hold
  the value they rule out. That is `no_value`, as is an unknown start with no sample at all.
- Nothing is evaluated after the recording ends; a window that opens after the end is
  `window_past_end`.

### 4.3 Verdicts up the chain

- **Check:** `fail` if any occurrence failed; else `inconclusive` if any was inconclusive; else
  `not_exercised` if there was no unskipped anchor; else `pass`.
- **Case:** the worst of its checks, in the order `fail` > `inconclusive` > `not_exercised` >
  `pass`. A case with no checks is `not_judgeable`.
- If the recording's event list was truncated (signal-loom caps it), integrity cannot be confirmed:
  every `pass` occurrence becomes `inconclusive` with reason `integrity_unknown`. Fails stay fails.

### 4.4 Worked example — the contract fixture

Synthetic. `BrakeCmd` (0x100) carries `BrakePedal`; `Lamps` (0x200) carries `BrakeLamp`; both every
10 ms. Check: when `BrakePedal == 1`, `BrakeLamp == 1` eventually within `[0, 100]` ms.

| Time | What happens | Occurrence verdict |
|---|---|---|
| 0 | pedal 0, lamp 0 | — (first sample, not an anchor) |
| 1000 ms | pedal → 1; lamp → 1 at 1040 ms | **pass**, evidence 1040 ms, value 1 |
| 2000 ms | pedal → 0; lamp → 0 at 2030 ms | — |
| 3000 ms | pedal → 1; lamp stays 0 | **fail**, window 3000–3100 seen, none true |
| 4000 ms | pedal → 0 | — |
| 5000 ms | pedal → 1; no `Lamps` frame from 4990 to 5200 ms (last at 4980 ms) | **inconclusive**, `integrity`, timeout at 4980 + 2.5 × 10 = 5005 ms |
| 6000 ms | pedal → 0 | — |
| 9990 ms | pedal → 1; lamp stays 0 | **inconclusive**, `window_past_end` (log ends 10 000 ms) |

Without the silence rule, the 5000 ms row would be a **fail**: the lamp holds 0 from 4980 ms and no
sample contradicts it. That is exactly the wrong answer the rule exists to prevent.

Check verdict: **fail** (1 pass, 1 fail, 2 inconclusive, 0 skipped). The same fixture with a second
check `never BrakeLamp == 1 within [0, 100]` gives that check **fail** at 1040 ms; a check whose
`when` is `BrakePedal == 7` gives **not_exercised**.

---

## 5. The verdict file — `loom-verdict/1`

```json
{
  "format": "loom-verdict/1",
  "tool": "loom-judge 1.0.0",
  "request_sha256": "…",
  "log": {"file": "brake.asc", "sha256": "…", "t_start_us": 0, "t_end_us": 10000000,
          "frames": 2000, "skipped": 0, "events_truncated": false},
  "dbc": [{"file": "brake.dbc", "sha256": "…", "channel": 0}],
  "cases": [
    {"case_id": "TC-1", "verdict": "fail",
     "checks": [
       {"id": "c1", "verdict": "fail",
        "anchors": 4, "skipped": 0, "pass": 1, "fail": 1, "inconclusive": 2,
        "occurrences": [
          {"anchor_us": 1000000, "verdict": "pass", "t_us": 1040000, "value": 1},
          {"anchor_us": 3000000, "verdict": "fail", "t_us": 3100000, "value": 0},
          {"anchor_us": 5000000, "verdict": "inconclusive", "reason": "integrity",
           "event": {"t_us": 5005000, "kind": "timeout"}},
          {"anchor_us": 9990000, "verdict": "inconclusive", "reason": "window_past_end"}
        ]}
     ]}
  ],
  "counts": {"cases": 1, "pass": 0, "fail": 1, "inconclusive": 0,
             "not_exercised": 0, "not_judgeable": 0}
}
```

- `occurrences` keeps the first 50 per check, in time order; `occurrences_truncated: true` says
  more existed. The counts are always complete.
- The file is **deterministic**: the same request, recording and DBC give byte-identical output.
  No run time, no host name, no absolute path (`file` is the base name).
- Exit codes: `0` judged (whatever the verdicts), `2` request refused, `1` a file could not be
  read.
- `pass_already_held` is written on every check, right after `inconclusive`, including when it is 0.
- `already_held` is written on an occurrence only when it is true, and is absent otherwise.

---

## 6. Cards — signal-loom

Order: SL-10 → SL-11 → SL-12 → SL-13 → SL-14 → SL-15. SL-16 and SL-17 are independent.
Every card: test first, shown failing; `cargo test -p loom-core` and the CI jobs green; no change
to the UI unless the card says so.

| Card | Size | What | Done when |
|---|---|---|---|
| **SL-10** Signal lookup by message and name | [S] | In `loom-core`, resolve `(message name, signal name)` to a signal, using the DBC names — including a signal whose display name became `Name@<id>`, and a multiplexed signal. An unknown pair is an error naming the pair. | Tests: plain, duplicate name in two messages, multiplexed, unknown. |
| **SL-11** Integrity events with a kind | [S] | Keep the existing labels, and store beside each checksum and counter event its kind, message id and signal. Timeouts are recomputed by the judge (§4.2), so they need nothing here. Readable inside `loom-core` only. | Tests: one of each kind on a synthetic log; the label text is unchanged. |
| **SL-12** Request types and refusal | [S] | `judge` module: serde types for §3, `deny_unknown_fields`, and a check that returns **every** problem of §3.3 at once. Signals and labels checked against the loaded DBC. | One test per refusal rule, plus one request with three problems that lists all three. |
| **SL-13** Evaluator | [M] **needs a capable model** | §4 exactly, on full-resolution replay of only the signals involved. The edge rule, held values, the three modes, the certain-decision rule, the three inconclusive reasons in order, `integrity_unknown`. | The §4.4 table as one test per row, built from a fixture generated in the test; plus `always` and `never` cases of each outcome; plus a `given` that is false, and one that is unknown; plus a sample exactly on each window edge (both edges are inside). Silence uses the timeout check's own cycle time and factor — read them from where that check gets them, do not re-derive. *Why capable:* the replay internals are the subtle part of this codebase, and an off-by-one at a window edge passes casual tests. |
| **SL-14** Verdict file | [S] | §5: the writer, the 50-occurrence cap, the counts, determinism. | Two runs give identical bytes; a check with 60 anchors keeps 50 and says truncated; counts sum. |
| **SL-15** `loom-judge` command | [S] | New workspace crate `crates/loom-judge`, a binary: `loom-judge --log FILE --dbc FILE [--dbc FILE@CHANNEL] --request FILE --out FILE`. Prints one summary line of the counts. Exit codes of §5. CI runs it on the contract fixture and compares with the committed expected verdict. | The CI step exists and fails when the expected file is edited. |
| **SL-16** Contract fixture | [S] | `fixtures/judge/`: a script that writes the §4.4 recording (SLOG or ASC), `brake.dbc`, `request.json`, and the hand-written `expected-verdict.json`. The expected file is written **from §4.4 by hand**, not produced by the judge. | Committed with a `SHA256SUMS`; grap-rust copies these files and checks the sums. |
| **SL-17** DBC dump for parity | [S] | `loom-judge --dump-dbc FILE` prints `loom-dbc-dump/1`: per signal, sorted by (id, start bit, signal name): message name, id with bit 31 cleared, signal name, start bit, length, byte order, signed, factor, offset, unit, multiplexer, multiplexed_by, labels. Min and max are dropped, because loom-core does not keep them and the judge does not use them. Replaces SL-05. | Test on `fixtures/hypercar_lap.dbc` against a committed expected file. |
| **SL-19** Integrity watch per channel | [S] | Timeouts keyed by (channel, id); checksums only on signals that decode on the frame's channel. Labels unchanged. | Tests: a silence on one channel is a timeout while another channel keeps sending; a bad checksum on channel 2 raises only the channel-2 copy; SLOG logs give the same events as before. |
| **SL-20** End-of-log timeout | [S] | `IntegrityWatch::finish` after the last record. For each cyclic `(channel, id)` still silent, threshold is `cycle * timeout_factor`; when `(t_end - last) > threshold`, raise `Timeout {name}` at `last + ceil(threshold)`, the same instant the judge uses for an ongoing silence. Events stay in time order. The event cap and `events_truncated` are unchanged. | Tests: a 2000 ms log where 0x200 (10 ms) stops at 980 ms has one timeout at 1005 ms; a stop at 1980 ms raises none; only the channel that stopped raises one; the hypercar lap event tests still pass. |

**SL-18** (later, [M]): open a verdict file in the app and show each occurrence on the event lane,
with the evidence sample under the playhead. Only after the owner has used SL-15 on real
recordings.

SL-03, SL-04 and SL-06 are unrelated to this plan and can go in any time.

## 7. Cards — grap-rust (summary; the full text is in grap-rust's
`docs/spec-procedure-runtime.md`, Amendment 2)

| Card | Size | What |
|---|---|---|
| **GJ-01** `checks` in the model's output | [S] | §5 schema: each case gets `checks`, required, may be `[]`, in the §3 grammar of this document. |
| **GJ-02** Validate checks | [S] | Each signal reference names a DBC `Signal` node whose key is in that case's `uses`; ops and modes from the lists; `within_ms` rule. A failing check is an issue for the refine loop. |
| **GJ-03** Export a judge request | [S] | From an emitted specification, write `loom-judge/1`. Cases with no checks are kept, so they are counted as not judgeable. |
| **GJ-04** Import verdicts | [S] | Read `loom-verdict/1`; check `request_sha256` against the exported file; record one event per case on the run, with the verdict, the log's sha256 and the counts. Never copy evidence values into a log line. |
| **GJ-05** DBC parity | [S] | Dump the same DBC through the grap-rust reader in SL-17's format; compare with signal-loom's expected file; any difference fails. |
| **GJ-06** First real judgement | [L] + owner | A real recording and a generated specification on the owner's machine. |

## 8. How the two repositories stay in step

- **Format names carry the version** (`loom-judge/1`, `loom-verdict/1`). A change that would make
  an old file mean something different is `/2`, and the old one is still read or refused by name.
- **One fixture, two copies.** signal-loom owns `fixtures/judge/`. grap-rust keeps a copy under its
  tests with the same `SHA256SUMS`; its tests read the expected verdict from there. Updating the
  contract means updating both, and a mismatch fails CI in grap-rust.
- **`tool` names the release.** It is `loom-judge <version>` from the workspace version, so every
  version bump rewrites `expected-verdict.json` and its `SHA256SUMS` line, and grap-rust copies
  both again. `tool` is provenance only; the format name says what the file means.
- **No shared code.** Each side implements its half from this document.

## 9. Decisions — defaults set, owner may change

| # | Question | Default |
|---|---|---|
| 1 | Is the first sample of a signal an anchor? | No: there is no earlier state to rise from. |
| 2 | One failed occurrence fails the check, even if ninety passed? | Yes. A requirement that holds sometimes does not hold. |
| 3 | Which requirements may carry checks? | Any; the model proposes and validation checks only references. A requirement observable only inside the ECU simply gets no checks and is counted as not judgeable. **The owner's AUTOSAR judgement is the better source here; a list of kinds that may never carry checks would sharpen this.** |
| 4 | Signal names: DBC names only? | Yes for `/1`. ARXML signal paths are resolved to DBC names inside grap-rust, not in the judge. |
| 5 | Real recordings in either repository? | Never. Fixtures are synthetic. Real recordings are judged on the owner's machine, and only the verdict file — which holds timestamps and decoded values, not frames — is imported. |
