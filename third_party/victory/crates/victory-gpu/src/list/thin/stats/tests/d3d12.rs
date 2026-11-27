use super::*;
use crate::host::rec::Rec;
use crate::list::thin::BLEND_FACTOR;
use std::cell::RefCell;

#[derive(Debug, PartialEq)]
enum Call {
    Stencil([u8; 4]),
    Blend(Option<[[u8; 4]; 4]>),
}

thread_local! {
    static CALLS: RefCell<Option<Vec<Call>>> = const { RefCell::new(None) };
}

// In test builds observe these two calls at the host D3D12 boundary.
// Forward through Rec's normal deref, including its barrier flush.
#[allow(non_snake_case)]
impl Rec {
    pub(crate) unsafe fn OMSetStencilRef(&self, value: u32) {
        // SAFETY: forwarded to the same open list with the caller's value.
        unsafe { (**self).OMSetStencilRef(value) };
        CALLS.with_borrow_mut(|log| {
            if let Some(log) = log {
                log.push(Call::Stencil(value.to_le_bytes()));
            }
        });
    }

    pub(crate) unsafe fn OMSetBlendFactor(&self, factor: Option<&[f32; 4]>) {
        // SAFETY: the caller's factor stays borrowed throughout the call.
        unsafe { (**self).OMSetBlendFactor(factor) };
        CALLS.with_borrow_mut(|log| {
            if let Some(log) = log {
                log.push(Call::Blend(factor.map(|f| f.map(f32::to_le_bytes))));
            }
        });
    }
}

#[test]
fn a_candidate_emits_the_same_d3d12_calls_in_order_with_identical_argument_bytes() {
    crate::host::host().expect("this equivalence test needs a D3D12 device");
    let factor = [0.125f32, -0.0, 0.75, 1.0];
    let [a, b, c, d] = factor.map(|v| u64::from(v.to_bits()));
    let run = |sample: Option<Ticket>| {
        let mut recording = Recording::new(0);
        recording.sample = sample;
        recording.inner.get_mut().unwrap().chunks.extend([
            vec![RESET | 1 << 16, 0, Plain::STENCIL_REF as u64 | 1 << 16, 19],
            vec![
                BLEND_FACTOR | 3 << 16,
                1,
                a | b << 32,
                c | d << 32,
                Plain::STENCIL_REF as u64 | 1 << 16,
                7,
                CLOSE,
            ],
        ]);
        CALLS.replace(Some(Vec::new()));
        recording.run_with_split(false);
        assert!(recording.done());
        CALLS.take().unwrap()
    };
    let baseline = run(None);
    assert_eq!(
        baseline,
        [
            Call::Stencil(19u32.to_le_bytes()),
            Call::Blend(Some(factor.map(f32::to_le_bytes))),
            Call::Stencil(7u32.to_le_bytes()),
        ]
    );
    let first = Ticket::default();
    assert_eq!(run(Some(first.clone())), baseline);
    let second = Ticket::default();
    assert_eq!(run(Some(second.clone())), baseline);
    let mut frames = Frames::default();
    frames.pending.push_back((1, vec![first, second]));
    assert_eq!(frames.take_ready()[0].reuse_candidate_lists, 1);
}
