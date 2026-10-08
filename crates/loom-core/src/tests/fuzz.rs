use crate::index::IndexedLog;
use crate::map::SignalMap;

#[test]
fn parsers_do_not_panic_on_garbage() {
    let mut state = 0x516C4F4Du64;
    let mut next = |len: usize| {
        let mut bytes = vec![0u8; len];
        for byte in &mut bytes {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            *byte = (state >> 33) as u8;
        }
        bytes
    };
    let mut samples = vec![
        Vec::new(),
        b"SLB1".to_vec(),
        b"LOGG".to_vec(),
        b"SLOGv1\nF not a frame\n".to_vec(),
        b"date\nbase hex\nnope\n".to_vec(),
        b"(1.0) not#zz\n".to_vec(),
    ];
    for n in 0..48 {
        let mut bytes = next(32 + (n * 97) % 3000);
        if n % 5 == 0 {
            bytes.splice(0..0, b"LOGG".iter().copied());
        }
        if n % 5 == 1 {
            bytes.splice(0..0, b"SLB1".iter().copied());
        }
        samples.push(bytes);
    }
    for bytes in samples {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = crate::scan::sniff(&bytes);
            let refused = IndexedLog::open_bytes(bytes.clone(), None).is_err();
            if let Ok(text) = String::from_utf8(bytes.clone()) {
                let _ = crate::dbc::parse(&text);
                let _ = SignalMap::parse(&text);
            }
            refused
        }));
        assert_eq!(
            result.ok(),
            Some(true),
            "garbage must be refused, and no parser may panic"
        );
    }
}
