//! Synthetic title calls, using the recording workers. In diagnostics builds
//! the independent D3D12 recorder also captures the commands sent to WARP.

use super::*;
use std::path::Path;
use std::time::Duration;

// The workers and the environment switches are process-wide. Each mode gets
// a fresh test process, including a fresh recorder and back-list pool.
#[test]
fn translation_overlaps_recording_and_preserves_the_command_stream() {
    const OUTPUT: &str = "VICTORY_TEST_THIN_OUTPUT";
    if let Some(output) = std::env::var_os(OUTPUT) {
        exercise(Path::new(&output));
        return;
    }
    let folder = tempfile::tempdir().unwrap();
    let name = std::thread::current().name().unwrap().to_owned();
    let run = |mode| {
        let path = folder.path().join(mode);
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture"])
            .env(OUTPUT, &path)
            .env("VICTORY_THIN_EARLY", mode)
            .env("VICTORY_D3D12_WARP", "1")
            .env("VICTORY_RECORD", folder.path())
            .env("VICTORY_RECORD_ALWAYS", "1")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "mode {mode}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        std::fs::read(path).unwrap()
    };
    assert_eq!(run("0"), run("1"));
}

fn until(ready: impl Fn() -> bool) {
    let start = Instant::now();
    while !ready() {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "translation did not advance"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn exercise(output: &Path) {
    let measure = std::env::var("VICTORY_THIN_EARLY").unwrap() == "1";
    start();
    let null = std::ptr::null();
    let mut title = list::boxed(0, null, true);
    let this: This = (&raw mut *title).cast();
    title.thin.begin(0, null);
    let abandoned = title.thin.recording.as_ref().unwrap().clone();
    stencil_ref(this, 201);
    list_close(this);
    let allocator = std::ptr::NonNull::<Object>::dangling().as_ptr();
    assert_eq!(list_reset(this, allocator, null), S_OK);
    assert!(abandoned.abandoned.load(Acquire));
    assert!(abandoned.first_submit.get().is_none());

    let recording = title.thin.recording.as_ref().unwrap().clone();
    let a = allocate_upload(this, 4, 16);
    assert!(!a.is_null());
    for i in 0..510 {
        stencil_ref(this, i % 256);
    }
    // RESET and UPLOAD plus these 510 calls are exactly CHUNK words. The
    // worker must finish them while the front is still open and unsubmitted.
    until(|| held(&recording.inner).kept.len() == 1);
    assert!(!recording.done());
    assert!(recording.first_submit.get().is_none());

    let mut factor = [0.25, 0.5, 0.75, 1.0];
    blend_factor(this, &factor);
    factor[0] = 0.875;
    blend_factor(this, &factor);
    let b = allocate_upload(this, 4, 16);
    assert!(!b.is_null());
    list_close(this);
    // Close must dispatch even this short tail without an Execute.
    until(|| recording.done());
    // The title can still populate uploads after Close.
    let payloads = [[1u8, 2, 3, 4], [9, 8, 7, 6]];
    // SAFETY: both returned blocks have four writable bytes.
    unsafe {
        a.cast::<[u8; 4]>().write(payloads[0]);
        b.cast::<[u8; 4]>().write(payloads[1]);
    }
    assert_eq!(
        held(&recording.inner).intervals.len(),
        if measure { 2 } else { 0 }
    );
    let lists = submitted(&[this]);
    let first = recording.first_submit.get().copied();
    assert_eq!(first.is_some(), measure);
    let repeated = submitted(&[this]);
    assert_eq!(recording.first_submit.get().copied(), first);
    let back = lists[0].back();
    assert_eq!(repeated[0].back(), back);
    let (count, before, total) = early::take();
    assert_eq!(count, u64::from(measure));
    assert_eq!(before, total, "Close completed before submit");
    assert_eq!(!total.is_zero(), measure);
    assert!(held(&recording.inner).intervals.is_empty());
    // SAFETY: this completed back list belongs to the recording we hold.
    let mut uploads = held(unsafe { &(*back.cast::<List>()).uploads });
    assert_eq!(uploads.len(), 2);
    let mut bytes = Vec::new();
    for ((upload, ptr), payload) in uploads.iter_mut().zip([a, b]).zip(payloads) {
        let start = upload.start();
        assert_eq!(start.wrapping_add(start.align_offset(16)), ptr);
        // SAFETY: ptr is the aligned four-byte range belonging to this upload.
        let saved = unsafe { ptr.cast::<[u8; 4]>().read() };
        assert_eq!(saved, payload);
        bytes.extend(saved);
    }
    #[cfg(feature = "diagnostics")]
    {
        let host = list::host_list(back).expect("D3D12 required for the capture");
        let commands = crate::replay::closed_list_bytes(&host.list).expect("complete D3D12 recording");
        assert!(
            commands.len() > 510 * 4,
            "expected stencil and blend calls on D3D12"
        );
        bytes.extend_from_slice(&commands);
    }
    std::fs::write(output, bytes).unwrap();
}
