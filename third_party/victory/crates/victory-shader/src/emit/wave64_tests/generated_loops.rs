//! Run with: cargo test -p victory-shader generated_loop_sweep -- --ignored --nocapture
//! Set WAVE64_LOOP_CASE to 0..259 to replay one dispatch.

use super::*;

// For each partner distance, s0 selects 0..64 lanes to enter the loop.
// Each lane exits after 1..8 rounds. Inside the loop it reads its partner's
// previous value, then all lanes read their partner once more after reconvergence.
fn loop_program(partner_xor: u32) -> NativeProgram {
    let mut w = vec![
        vop2(0x25, 1, k(1), 0),           // increment = lane + 1
        vop2(0x1D, 2, k(partner_xor), 0), // partner lane
        vop2(0x1A, 2, k(2), 2),           // its byte address
        vop1(0x01, 3, v(1)),              // initial value
        vop2(0x16, 4, k(3), 0),
        vop2(0x1D, 4, v(0), 4),
        vop2(0x1B, 4, k(7), 4),
        vop2(0x25, 4, k(1), 4), // remaining = 1 + ((lane ^ (lane >> 3)) & 7)
        vop1(0x01, 5, k(0)),    // completed rounds
        vop1(0x01, 6, k(42)),   // last read (sentinel for lanes that skip)
        vop2(0x1D, 8, k(partner_xor), 0),
        vopc(0xC4, 0, 8),    // enter if (lane ^ partner_xor) < s0
        sop1(0x24, 12, VCC), // save full EXEC
    ];
    let mut body = ds(0xB3, 6, 2, 3, 0).to_vec(); // ds_bpermute_b32
    body.extend([
        vop2(0x25, 3, v(1), 6), // value = increment + last read
        vop2(0x25, 5, k(1), 5),
        vop2(0x25, 4, 193, 4), // remaining--
        vopc(0xC5, k(0), 4),
        sop2(0x0F, EXEC, EXEC, VCC), // keep unfinished lanes
    ]);
    let loop_words = (body.len() + 1) as i16;
    w.push(sopp(0x08, loop_words)); // execz: skip empty loop
    w.extend(body);
    w.push(sopp(0x09, -loop_words)); // execnz: another round
    w.push(sop1(0x04, EXEC, 12));
    w.extend(ds(0xB3, 7, 2, 3, 0));
    w.extend(stores([3, 5, 6, 7]));
    native(&w)
}

fn expected(partner_xor: usize, cutoff: usize) -> [[u32; 4]; 64] {
    let mut values: [u32; 64] = std::array::from_fn(|lane| lane as u32 + 1);
    let rounds = |lane: usize| {
        if (lane ^ partner_xor) < cutoff {
            1 + ((lane ^ (lane >> 3)) & 7)
        } else {
            0
        }
    };
    let mut reads = [42; 64];
    for step in 0..8 {
        let previous = values;
        for lane in 0..64 {
            if step < rounds(lane) {
                let src = lane ^ partner_xor;
                reads[lane] = if step < rounds(src) { previous[src] } else { 0 };
                values[lane] = reads[lane] + lane as u32 + 1;
            }
        }
    }
    std::array::from_fn(|lane| {
        [
            values[lane],
            rounds(lane) as u32,
            reads[lane],
            values[lane ^ partner_xor],
        ]
    })
}

#[test]
#[ignore = "explicit GPU sweep; requires a device accepting 32-lane programs"]
fn generated_loop_sweep() {
    let replay = std::env::var_os("WAVE64_LOOP_CASE").map(|raw| {
        let case: usize = raw
            .to_str()
            .unwrap()
            .parse()
            .expect("invalid WAVE64_LOOP_CASE");
        assert!(case < 260, "WAVE64_LOOP_CASE must be in 0..260");
        case
    });
    let _one = super::super::valu_tests::ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let host = Host::new(false).expect("GPU required for loop sweep");
    for (group, partner_xor) in [1, 16, 32, 63].into_iter().enumerate() {
        if replay.is_some_and(|case| case / 65 != group) {
            continue;
        }
        let shader = paired(&loop_program(partner_xor));
        let (root, pipeline) = host
            .compute_pipeline(&shader)
            .expect("paired loop pipeline");
        let arg = shader
            .user_data
            .iter()
            .find(|u| matches!((u.root, u.slot, u.dword), (RootType::Constants, 0, 0)))
            .expect("s0 cutoff");
        for cutoff in 0..=64 {
            let case = group * 65 + cutoff;
            if replay.is_some_and(|id| id != case) {
                continue;
            }
            let mut constants = vec![0; shader.user_data_dwords() as usize];
            constants[arg.index as usize] = cutoff as u32;
            let output = host
                .dispatch(
                    &root,
                    &pipeline,
                    &Dispatch {
                        roots: vec![(0, Root::Constants(constants))],
                        views: &[View::RawUav(256)],
                        samplers: &[],
                        groups: [1, 1, 1],
                    },
                )
                .unwrap_or_else(|e| panic!("case {case}: {e}"));
            let want = expected(partner_xor as usize, cutoff);
            assert_eq!(output[0].len(), 256);
            for (lane, actual) in output[0].chunks_exact(4).enumerate() {
                assert_eq!(actual, want[lane], "WAVE64_LOOP_CASE={case}, lane {lane}");
            }
        }
    }
}
