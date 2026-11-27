use super::*;
use crate::list::thin::{CLOSE, Plain, RESET};

mod d3d12;

fn ticket_for(kind: u32, chunks: &[Vec<u64>]) -> Ticket {
    let t = Ticket::default();
    complete(&t, kind, chunks, Timing::default());
    t
}

#[test]
fn stream_identity_includes_named_objects_and_ignores_chunk_boundaries() {
    let chunks = [
        vec![RESET | 1 << 16, 0],
        vec![Plain::COPY_RESOURCE as u64 | 2 << 16, 123, 456, CLOSE],
    ];
    let key = |kind, chunks: &[Vec<u64>]| ticket_for(kind, chunks).sample.get().unwrap().key;
    let first = key(0, &chunks);
    assert_eq!(key(0, &[chunks.concat()]), first);
    assert_ne!(key(1, &chunks), first);
    let mut renamed = chunks.clone();
    renamed[1][2] += 1;
    assert_ne!(key(0, &renamed), first);
    let with_upload = [vec![UPLOAD | 1 << 16, 123, CLOSE]];
    assert_ne!(key(0, &with_upload), key(0, &with_upload));
}

#[test]
fn frame_statistics_wait_for_tickets_and_count_prior_frames() {
    let mut frames = Frames::default();
    let late = Ticket::default();
    let first = ticket_for(0, &[vec![CLOSE]]);
    frames
        .pending
        .push_back((1, vec![late.clone(), first.clone()]));
    frames.pending.push_back((2, vec![first]));
    frames.pending.push_back((3, vec![]));
    assert!(frames.take_ready().is_empty());
    let mut timing = Timing::default();
    timing.add(Duration::from_micros(42), Some(3));
    complete(&late, 0, &[vec![RESET | 1 << 16, 0, CLOSE]], timing);
    let rows = frames.take_ready();
    assert_eq!(rows[0].lists, [(3, 42), (1, 0)]);
    assert_eq!(rows[0].sizes(), (3, 2.0));
    assert_eq!(
        (
            rows[0].repeated,
            rows[0].split_chunks,
            rows[0].root_bindings
        ),
        (0, 1, 3)
    );
    assert_eq!((rows[1].frame, rows[1].repeated), (2, 1));
    assert_eq!(rows[2].sizes(), (0, 0.0));
    assert!(frames.pending.is_empty());
}

#[test]
fn fingerprints_cover_copied_views_and_active_ffi_fields() {
    use crate::list::thin::{RECORD, TARGETS};
    let key = |words: &[u64]| ticket_for(0, &[words.to_vec()]).sample.get().unwrap().key;
    let mut words = vec![BARRIER | (5 << 16), 1, 0, 42, 3 << 32, 8];
    let first = key(&words);
    words[5] |= 123 << 32; // transition padding
    assert_eq!(key(&words), first);
    words[3] += 1;
    assert_ne!(key(&words), first);

    let step = size_of::<CopyLocation>() / 8;
    let mut copy = vec![0; 3 + 2 * step];
    copy[0] = COPY_TEXTURE | ((copy.len() - 1) as u64) << 16;
    let first = key(&copy);
    copy[4] = 17 << 32; // padding after kind
    copy[6] = 888; // inactive footprint in a subresource location
    assert_eq!(key(&copy), first);
    copy[5] = 9; // subresource index
    assert_ne!(key(&copy), first);

    let mut view = vec![0; 4 + 2 * RECORD];
    view[0] = TARGETS | ((view.len() - 1) as u64) << 16;
    view[1] = 1;
    view[2] = 1;
    let first = key(&view);
    view[5] = 42; // named resource in the recorded RTV
    assert_ne!(key(&view), first);
    let first = key(&view);
    view[6] = 1; // changed descriptor on the same resource
    assert_ne!(key(&view), first);
}

#[test]
fn history_expires_after_the_last_occurrence_leaves_the_window() {
    let mut frames = Frames::default();
    let key = [1; 16];
    let now = Instant::now();
    assert!(!frames.remember(key, now, now).0);
    assert!(frames.remember(key, now, now).0);
    for _ in 0..HISTORY - 1 {
        frames.remember([2; 16], now, now);
    }
    assert!(frames.finishes.contains_key(&key));
    frames.remember([2; 16], now, now);
    assert!(!frames.remember(key, now, now).0);
}

#[test]
fn reuse_estimate_charges_only_new_translations_with_an_available_match() {
    let start = Instant::now();
    let ms = Duration::from_millis;
    let translated = |kind, from, chunks: &[(u64, u64)], words: &[u64]| {
        let mut t = Ticket::default();
        let mut timing = Timing::default();
        for &(_, work) in chunks {
            timing.add(ms(work), None);
        }
        complete(&t, kind, &[words.to_vec()], timing);
        let recording = Arc::get_mut(&mut t).unwrap();
        recording.began = start + ms(from);
        let (offset, work) = chunks.last().unwrap();
        recording.sample.get_mut().unwrap().finished = recording.began + ms(offset + work);
        t
    };
    let a = [RESET | 1 << 16, 0, CLOSE];
    let upload = [UPLOAD | 1 << 16, 1, CLOSE];
    let mut frames = Frames::default();
    // The second worker finishes first. The third can reuse its result
    // while the first worker is still busy. Its gaps between chunks are free.
    let first = translated(0, 0, &[(0, 90)], &a);
    let second = translated(0, 1, &[(0, 3)], &a);
    let third = translated(0, 5, &[(0, 2), (10, 4)], &a);
    frames.pending.push_back((1, vec![first, second]));
    frames.pending.push_back((
        2,
        vec![
            third.clone(),
            third,
            translated(1, 20, &[(0, 1)], &a),
            translated(0, 21, &[(0, 1)], &upload),
            translated(0, 22, &[(0, 1)], &upload),
        ],
    ));
    let rows = frames.take_ready();
    assert_eq!(
        (rows[0].prior_unready_lists, rows[0].reuse_candidate_lists),
        (1, 0)
    );
    assert_eq!(rows[0].translation, ms(93));
    assert_eq!(rows[0].prior_unready_work, ms(3));
    assert_eq!((rows[1].repeated, rows[1].resubmitted_lists), (2, 1));
    assert_eq!(
        (rows[1].reuse_candidate_lists, rows[1].reuse_candidate_work),
        (1, ms(6))
    );
    assert_eq!(rows[1].translation, ms(9));
}
