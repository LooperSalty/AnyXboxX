//! One wave, two consecutive swizzles, and a distinct EXEC mask for each instruction.
//! Run with: cargo test -p victory-shader generated_swizzle_sweep -- --ignored --nocapture
//! Set SWIZZLE_CASE to a case number (0..255) to replay only that program.

use super::*;

// Each odd multiplier permutes all six bits' worth of lane IDs modulo 64.
// The cutoff selects exactly that many lanes, including empty and full masks.
fn set_mask(words: &mut Vec<u32>, multiplier: u32, key: u32, cutoff: u32) {
    words.extend(vop3(0x169, 4, v(0), k(multiplier))); // v_mul_lo_u32
    words.push(vop2(0x1B, 4, k(63), 4)); // v_and_b32
    words.push(vop2(0x1D, 4, k(key), 4)); // v_xor_b32
    words.push(vopc(0xC4, k(cutoff), 4)); // v_cmp_gt_u32
    words.push(sop1(0x24, 12, VCC)); // save EXEC and select active lanes
}

#[test]
#[ignore = "explicit GPU sweep; requires a device accepting 32-lane programs"]
fn generated_swizzle_sweep() {
    let _one = super::super::valu_tests::ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let host = Host::new(false).expect("GPU required for swizzle sweep");
    let cases = match std::env::var("SWIZZLE_CASE") {
        Ok(raw) => {
            let case = raw.parse::<u32>().expect("SWIZZLE_CASE must be an integer");
            assert!(case < 256, "SWIZZLE_CASE must be in 0..256");
            case..=case
        }
        Err(std::env::VarError::NotPresent) => 0..=255,
        Err(e) => panic!("invalid SWIZZLE_CASE: {e}"),
    };
    for case in cases {
        let quad = case; // all four selectors for each of the four positions
        let bits = (case * 15401 + 731) & 0x7fff;
        let m = 1 + 2 * (case % 32);
        let key = (case * 13) % 64;
        let count = case % 65;
        let second_count = (case * 17 + 23) % 65;
        let mut words = vec![vop2(0x25, 1, k(1), 0), vop1(0x01, 2, k(42))];
        set_mask(&mut words, m, key, count);
        words.extend(ds(0x35, 1, 1, 0, 0x8000 | quad)); // in place
        words.push(sop1(0x04, EXEC, 12));
        set_mask(&mut words, 64 - m, key ^ 63, second_count);
        words.extend(ds(0x35, 2, 1, 0, bits));
        words.push(sop1(0x04, EXEC, 12));
        words.push(vop2(0x1A, 15, k(3), 0)); // two output dwords per lane
        words.extend(store(1, 15, 0));
        words.extend(store(2, 15, 4));
        let n = native(&words);
        let p = ir::build(&n.code, true, n.vgprs).unwrap();
        let shader = lowered(&n, &p, &HashSet::new()).unwrap();
        let (root, pipeline) = host
            .compute_pipeline(&shader)
            .unwrap_or_else(|e| panic!("case {case}: {e}"));
        let output = host
            .dispatch(
                &root,
                &pipeline,
                &Dispatch {
                    roots: vec![(
                        0,
                        Root::Constants(vec![0; shader.user_data_dwords().max(1) as usize]),
                    )],
                    views: &[View::RawUav(128)],
                    samplers: &[],
                    groups: [1, 1, 1],
                },
            )
            .unwrap();

        let active = |lane: u32| ((lane * m % 64) ^ key) < count;
        let second = |lane: u32| ((lane * (64 - m) % 64) ^ (key ^ 63)) < second_count;
        let after_quad = |lane: u32| {
            let src = (lane & !3) + ((quad >> (2 * (lane % 4))) & 3);
            if !active(lane) {
                lane + 1
            } else if active(src) {
                src + 1
            } else {
                0
            }
        };
        for lane in 0..64 {
            let within = (((lane & 31) & bits) | ((bits >> 5) & 31)) ^ (bits >> 10);
            let src = (lane & 32) + within;
            let value = if !second(lane) {
                42
            } else if second(src) {
                after_quad(src)
            } else {
                0
            };
            let offset = lane as usize * 2;
            assert_eq!(
                output[0][offset..offset + 2],
                [after_quad(lane), value],
                "case {case}, lane {lane}, quad {quad:#04x}, bitmask {bits:#06x}"
            );
        }
    }
}
