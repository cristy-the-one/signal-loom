use super::support::*;
use crate::session::Session;

#[test]
fn hypercar_lap_decodes_like_a_drive() {
    use sha2::{Digest, Sha256};
    let lap = std::fs::read(lap_fixture("hypercar_lap.slog")).expect("synthetic lap");
    assert_eq!(
        lap.len(),
        7_699_372,
        "regenerate with scripts/gen_fixture.py"
    );
    let digest = Sha256::digest(&lap);
    let sha = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        sha,
        "807e35542bd5fe4599f25da0991ca8a2ee2a50f301a3910615c194aa5556b6c3"
    );
    let mut session = Session::new();
    session
        .open_path(&lap_fixture("hypercar_lap.slog"))
        .expect("synthetic lap");
    let summary = session.summary().unwrap();
    assert!(summary.t_end_us > 590_000_000, "{}", summary.t_end_us);
    assert!(summary.frame_count > 180_000, "{}", summary.frame_count);
    assert!(
        summary
            .map_label
            .as_deref()
            .unwrap_or("")
            .contains("hypercar_lap"),
        "{:?}",
        summary.map_label
    );
    let labels: Vec<&str> = summary
        .events
        .iter()
        .map(|event| event.label.as_str())
        .collect();
    for needle in [
        "DTC P0301",
        "Bus-off",
        "ECM_Fast missing",
        "ABS checksum",
        "Error frame",
        "ABS",
        "Upshift",
        "Checksum ",
        "Timeout ",
        "Counter ",
    ] {
        assert!(
            labels.iter().any(|label| label.contains(needle)),
            "missing {needle} in {labels:?}"
        );
    }
    // The planted faults, by their times in hypercar_bus.py. The decoder's own
    // events (Counter, Timeout, Checksum) must fall in the same second.
    let near = |prefix: &str, t_us: u64| {
        let found = summary
            .events
            .iter()
            .any(|event| event.label.starts_with(prefix) && event.t_us.abs_diff(t_us) <= 1_000_000);
        assert!(found, "no {prefix:?} within 1 s of {t_us} us");
    };
    near("DTC P0301", 125_000_000);
    near("TCU counter skip", 200_000_000);
    near("Counter ", 200_000_000);
    near("ECM_Fast missing", 250_000_000);
    near("Timeout ECM_Fast", 250_000_000);
    near("Bus-off", 410_000_000);
    near("ABS checksum", 460_000_000);
    near("Checksum ", 460_000_000);
    let at = |t: u64, name: &str| {
        session
            .values_at(t)
            .unwrap()
            .into_iter()
            .find(|value| value.name == name)
            .map(|value| value.value)
            .unwrap_or(f64::NAN)
    };
    assert!(at(2_000_000, "VehicleSpeed") < 1.0);
    assert!(at(200_000_000, "VehicleSpeed") > 150.0);
    assert!(at(200_000_000, "Gear") >= 5.0);
    assert!(at(200_000_000, "EngineRPM") < 9000.0);
    assert!(at(200_000_000, "BrakePressure") < 0.5);
    assert!(at(299_000_000, "BrakePressure") > 40.0);
    assert!(at(500_000_000, "CoolantTemp") > at(5_000_000, "CoolantTemp") + 20.0);
    assert!(at(299_000_000, "AbsActive") > 0.5);
    assert!(at(560_000_000, "DoorFL") > 0.5);
}

#[test]
fn signals_carry_their_decode_step() {
    let mut session = Session::new();
    session
        .open_path(&lap_fixture("hypercar_lap.slog"))
        .expect("synthetic lap");
    let summary = session.summary().unwrap();
    let step = |name: &str| {
        summary
            .signals
            .iter()
            .find(|signal| signal.name == name)
            .unwrap_or_else(|| panic!("{name}"))
            .step
    };
    assert_eq!(step("VehicleSpeed"), Some(0.01));
    assert_eq!(step("EngineRPM"), Some(0.25));
    assert_eq!(step("BrakePressure"), Some(0.5));
    assert_eq!(step("Gear"), Some(1.0));

    let csv = b"t_us,signal,value,unit\n0,Speed,1.5,km/h\n";
    let mut decoded = Session::new();
    decoded.open_bytes("decoded.csv", csv.to_vec()).unwrap();
    assert_eq!(decoded.summary().unwrap().signals[0].step, None);
}

#[test]
fn deck_bound_signals_are_live_mid_drive() {
    // Names `readingsFrom` in src/gauges.ts looks up. A rename in the
    // generator or DBC leaves the cluster drawing blanks.
    let names = [
        "VehicleSpeed",
        "EngineRPM",
        "DisplayedRPM",
        "CoolantTemp",
        "OilTemp",
        "Soc",
        "FuelLevel",
        "Gear",
        "GearActual",
        "MilLamp",
        "TelltaleMil",
        "AbsActive",
        "TelltaleAbs",
        "TurnLeft",
        "TelltaleLeft",
        "TurnRight",
        "TelltaleRight",
        "DoorFL",
        "EscActive",
    ];
    let mut session = Session::new();
    session
        .open_path(&lap_fixture("hypercar_lap.slog"))
        .expect("synthetic lap");
    let summary = session.summary().unwrap();
    let have: std::collections::HashSet<&str> = summary
        .signals
        .iter()
        .map(|signal| signal.name.as_str())
        .collect();
    for name in names {
        assert!(have.contains(name), "lap is missing {name}");
    }
    // 4:58, the deck playhead: braking off the straight, MIL already set.
    let held = session.values_at(298_000_000).unwrap();
    for name in names {
        let value = held.iter().find(|item| item.name == name);
        assert!(
            value.is_some_and(|item| item.value.is_finite()),
            "{name} did not decode at 4:58"
        );
    }
    let mil = held.iter().find(|item| item.name == "MilLamp").unwrap();
    assert!(mil.value > 0.5, "MIL should be lit mid-drive");
    let abs = held.iter().find(|item| item.name == "AbsActive").unwrap();
    assert!(abs.value > 0.5, "ABS should be lit on the straight's brake");
}
