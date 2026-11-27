//! One frame's D3D12 work recorded and run again, to find out what it costs
//! the card and to try changes on it in seconds (docs/replay.md).
//!
//! [`record`] hooks the host's D3D12 objects (`hook`) and keeps what a frame
//! needs; `snap` writes one frame down; [`play`] runs it. `wire` is what a
//! capture holds, `pso` pipeline descriptions, `states` the states textures
//! are in, `edit` the experiments, `gpu` helpers.

mod edit;
mod gpu;
mod hook;
pub mod play;
mod pso;
pub mod record;
mod snap;
mod states;
mod wire;

/// Serialized host commands of one closed synthetic list.
#[cfg(test)]
pub(crate) fn test_commands(
    list: &windows::Win32::Graphics::Direct3D12::ID3D12GraphicsCommandList,
    draws: usize,
) -> std::sync::Arc<Vec<u8>> {
    use record::recorder;
    use windows::core::Interface;
    use wire::Cmd;
    let recorder = recorder().expect("D3D12 command recorder");
    let state = recorder.list(list.as_raw()).expect("hooked D3D12 list");
    let bytes = state.closed().unwrap().0.expect("complete recording");
    let commands = Cmd::all(&bytes).expect("valid command stream");
    assert_eq!(
        commands
            .iter()
            .filter(|c| matches!(c, Cmd::DrawIndexed { .. }))
            .count(),
        draws
    );
    assert_eq!(
        commands
            .iter()
            .filter(|c| matches!(c, Cmd::CopyResource { .. }))
            .count(),
        draws.div_ceil(64)
    );
    assert!(commands.iter().any(|c| matches!(c, Cmd::Barriers { .. })));
    assert!(commands.iter().any(|c| matches!(c, Cmd::Constants { .. })));
    let problems = recorder.problems();
    assert!(problems.is_empty(), "{problems:?}");
    for (method, count) in record::not_recorded_calls() {
        if method.starts_with("command list method ") {
            assert_eq!(count, 0, "{method}");
        }
    }
    bytes
}
