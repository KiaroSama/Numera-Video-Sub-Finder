use super::*;

fn finish(transport: &mut Transport, request: &PreviewRequest, at: u64) -> bool {
    transport.finish(request, Some("source"), Some(at)).publish
}

#[test]
fn playback_uses_elapsed_time_while_slow_previews_coalesce() {
    let mut transport = Transport::default();
    transport.play(10_000, 0, 0, 10_000);
    assert_eq!(transport.tick(10_250, 10_000), Some((250, false)));
    let first = transport.take_request("source").unwrap();
    assert_eq!((first.at_ms, first.step), (250, 0));
    assert_eq!(transport.tick(10_500, 10_000), Some((500, false)));
    assert!(transport.take_request("source").is_none());
    assert_eq!(transport.tick(10_750, 10_000), Some((750, false)));
    assert!(finish(&mut transport, &first, 240));
    assert_eq!(transport.displayed(), Some(240));
    let latest = transport.take_request("source").unwrap();
    assert_eq!((latest.at_ms, latest.step), (750, 0));
    assert!(finish(&mut transport, &latest, 733));
    assert_eq!(transport.displayed(), Some(733));
    assert_eq!(transport.tick(11_500, 10_000), Some((1500, false)));
}

#[test]
fn continuous_slow_playback_publishes_frames_and_surfaces_current_failures() {
    let mut transport = Transport::default();
    transport.play(0, 0, 0, 10_000);
    transport.tick(250, 10_000);
    let mut request = transport.take_request("source").unwrap();
    let mut shown = Vec::new();
    for (ticks, completed_pts, latest) in [
        (vec![500], 240, 500),
        (vec![750, 1000], 480, 1000),
        (vec![1250], 960, 1250),
        (vec![1500, 1750], 1240, 1750),
    ] {
        // Every request takes 400 ms, spanning one or two 250 ms clock ticks.
        for tick in ticks {
            assert_eq!(transport.tick(tick, 10_000), Some((tick, false)));
            assert!(transport.take_request("source").is_none());
        }
        assert!(finish(&mut transport, &request, completed_pts));
        shown.push(transport.displayed().unwrap());
        request = transport.take_request("source").unwrap();
        assert_eq!(request.at_ms, latest);
    }
    assert_eq!(shown, [240, 480, 960, 1240]);
    assert_eq!(transport.tick(2000, 10_000), Some((2000, false)));
    let failed = transport.finish(&request, Some("source"), None);
    assert!(failed.released && failed.current && !failed.publish);
    assert!(!transport.has_pending());
    assert!(transport.take_request("source").is_none());
    assert_eq!(transport.tick(2250, 10_000), None);
    assert_eq!(transport.displayed(), Some(1240));
}

#[test]
fn pause_resume_and_manual_seek_reanchor_without_counting_paused_time() {
    let mut transport = Transport::default();
    transport.play(1000, 4000, 0, 20_000);
    assert_eq!(transport.pause(1450, 20_000), Some(4450));
    assert_eq!(transport.tick(9000, 20_000), None);
    let paused = transport.take_request("source").unwrap();
    assert_eq!(paused.at_ms, 4450);
    assert!(finish(&mut transport, &paused, 4440));
    transport.play(9000, 4440, 0, 20_000);
    assert_eq!(transport.position(), 4440);
    assert_eq!(transport.tick(9250, 20_000), Some((4690, false)));
    transport.seek(8000);
    assert_eq!(transport.tick(10_000, 20_000), None);
    let sought = transport.take_request("source").unwrap();
    assert_eq!(sought.at_ms, 8000);
    assert!(finish(&mut transport, &sought, 7991));
    transport.play(12_000, 7991, 0, 20_000);
    assert_eq!(transport.position(), 7991);
    assert_eq!(transport.tick(12_250, 20_000), Some((8241, false)));
}

#[test]
fn playback_clamps_at_the_end_and_does_not_move_back_on_a_clock_regression() {
    let mut transport = Transport::default();
    transport.play(1000, 9000, 0, 9999);
    assert_eq!(transport.tick(1250, 9999), Some((9250, false)));
    assert_eq!(transport.tick(1200, 9999), Some((9250, false)));
    assert_eq!(transport.tick(3000, 9999), Some((9999, true)));
    assert_eq!(transport.tick(4000, 9999), None);
    assert_eq!(transport.take_request("source").unwrap().at_ms, 9999);
}

#[test]
fn natural_end_restarts_after_the_last_frame_pts_is_published() {
    let mut transport = Transport::default();
    transport.play(1000, 5000, 2000, 5999);
    assert_eq!(transport.tick(2000, 5999), Some((5999, true)));
    let last = transport.take_request("source").unwrap();
    assert!(finish(&mut transport, &last, 5900));
    assert_eq!(transport.displayed(), Some(5900));
    assert_eq!(transport.play(3000, 5900, 2000, 5999), 2000);
    let restarted = transport.take_request("source").unwrap();
    assert_eq!((restarted.at_ms, restarted.step), (2000, 0));
    assert!(finish(&mut transport, &restarted, 2000));
    assert_eq!(transport.tick(3250, 5999), Some((2250, false)));
    transport.seek(5900);
    assert_eq!(transport.play(4000, 5900, 2000, 5999), 5900);
    assert_eq!(transport.tick(4050, 5999), Some((5950, false)));
}

#[test]
fn frame_steps_use_returned_pts_and_coalesce_repeated_pending_actions() {
    let mut transport = Transport::default();
    transport.seek(40);
    let seek = transport.take_request("source").unwrap();
    transport.step(1);
    transport.step(1);
    assert!(transport.take_request("source").is_none());
    assert!(finish(&mut transport, &seek, 33));
    let next = transport.take_request("source").unwrap();
    assert_eq!((next.at_ms, next.step), (33, 1));
    assert!(finish(&mut transport, &next, 100));
    assert!(transport.take_request("source").is_none());
    transport.step(-1);
    let previous = transport.take_request("source").unwrap();
    assert_eq!((previous.at_ms, previous.step), (100, -1));
    assert!(finish(&mut transport, &previous, 33));
    assert_eq!(transport.displayed(), Some(33));
}

#[test]
fn a_new_seek_supersedes_pending_frame_actions_and_old_images() {
    let mut transport = Transport::default();
    transport.seek(0);
    let old = transport.take_request("source").unwrap();
    transport.step(1);
    transport.seek(3000);
    transport.seek(7000);
    assert!(!finish(&mut transport, &old, 0));
    assert_eq!(transport.displayed(), None);
    let next = transport.take_request("source").unwrap();
    assert_eq!((next.at_ms, next.step), (7000, 0));
    assert!(finish(&mut transport, &next, 6980));
    assert!(transport.take_request("source").is_none());
}

#[test]
fn a_frame_step_during_playback_uses_the_displayed_frame_and_drops_clock_work() {
    let mut transport = Transport::default();
    transport.seek(100);
    let initial = transport.take_request("source").unwrap();
    assert!(finish(&mut transport, &initial, 100));
    transport.play(1000, 100, 0, 10_000);
    transport.tick(1250, 10_000);
    let playing = transport.take_request("source").unwrap();
    transport.tick(1500, 10_000);
    transport.step(1);
    assert!(!finish(&mut transport, &playing, 333));
    let step = transport.take_request("source").unwrap();
    assert_eq!((step.at_ms, step.step), (100, 1));
    assert!(finish(&mut transport, &step, 133));
    assert_eq!(transport.tick(2000, 10_000), None);
    assert!(!transport.has_pending());
}

#[test]
fn play_waits_for_an_inflight_frame_step_before_anchoring_to_its_pts() {
    let mut transport = Transport::default();
    transport.seek(33);
    let initial = transport.take_request("source").unwrap();
    assert!(finish(&mut transport, &initial, 33));
    transport.step(1);
    let next = transport.take_request("source").unwrap();
    transport.play(1000, 33, 0, 10_000);
    assert_eq!(transport.tick(1250, 10_000), None);
    assert!(transport.take_request("source").is_none());
    assert!(finish(&mut transport, &next, 5000));
    assert_eq!(transport.displayed(), Some(5000));
    assert_eq!(transport.tick(1500, 10_000), Some((5500, false)));
    let playing = transport.take_request("source").unwrap();
    assert_eq!((playing.at_ms, playing.step), (5500, 0));
}

#[test]
fn play_preserves_a_queued_frame_action_and_pause_can_cancel_its_anchor() {
    let mut transport = Transport::default();
    transport.seek(100);
    let initial = transport.take_request("source").unwrap();
    assert!(finish(&mut transport, &initial, 100));
    transport.play(0, 100, 0, 10_000);
    transport.tick(250, 10_000);
    let old = transport.take_request("source").unwrap();
    transport.tick(500, 10_000);
    transport.step(1);
    transport.play(600, 600, 0, 10_000);
    assert_eq!(transport.tick(850, 10_000), None);
    assert!(!finish(&mut transport, &old, 333));
    let next = transport.take_request("source").unwrap();
    assert_eq!((next.at_ms, next.step), (100, 1));
    assert_eq!(transport.pause(900, 10_000), Some(100));
    assert!(!finish(&mut transport, &next, 5000));
    let paused = transport.take_request("source").unwrap();
    assert_eq!((paused.at_ms, paused.step), (100, 0));
    assert!(finish(&mut transport, &paused, 100));
    assert_eq!(transport.tick(2000, 10_000), None);
}

#[test]
fn play_waits_for_both_active_and_queued_steps_before_using_the_final_pts() {
    let mut transport = Transport::default();
    transport.seek(100);
    let initial = transport.take_request("source").unwrap();
    assert!(finish(&mut transport, &initial, 100));
    transport.step(1);
    let first = transport.take_request("source").unwrap();
    transport.step(1);
    transport.play(1000, 100, 0, 20_000);
    assert!(finish(&mut transport, &first, 5000));
    let second = transport.take_request("source").unwrap();
    assert_eq!((second.at_ms, second.step), (5000, 1));
    assert_eq!(transport.tick(1250, 20_000), None);
    assert!(transport.take_request("source").is_none());
    assert!(finish(&mut transport, &second, 9000));
    assert_eq!(transport.displayed(), Some(9000));
    assert_eq!(transport.tick(1500, 20_000), Some((9500, false)));
    let playing = transport.take_request("source").unwrap();
    assert_eq!((playing.at_ms, playing.step), (9500, 0));
}

#[test]
fn source_reset_rejects_old_success_and_keeps_one_in_flight_owner() {
    let mut transport = Transport::default();
    transport.seek(5000);
    let old = transport.take_request("source").unwrap();
    transport.play(1000, 5000, 0, 9000);
    transport.step(1);
    transport.reset_source();
    assert_eq!(transport.displayed(), None);
    assert_eq!(transport.tick(2000, 9000), None);
    transport.seek(0);
    assert!(transport.take_request("replacement").is_none());
    let stale = transport.finish(&old, Some("replacement"), Some(5000));
    assert!(stale.released && !stale.current && !stale.publish);
    let new = transport.take_request("replacement").unwrap();
    assert_eq!((new.at_ms, new.step), (0, 0));
    let duplicate = transport.finish(&old, Some("replacement"), None);
    assert!(!duplicate.released && !duplicate.current && !duplicate.publish);
    assert!(transport.take_request("replacement").is_none());
    assert!(transport.finish(&new, Some("replacement"), Some(0)).publish);
}

#[test]
fn same_token_readmission_invalidates_the_old_epoch_and_errors_do_not_publish() {
    let mut transport = Transport::default();
    transport.seek(400);
    let old = transport.take_request("source").unwrap();
    transport.reset_source();
    assert!(!transport.finish(&old, Some("source"), Some(400)).current);
    transport.seek(50);
    let failed = transport.take_request("source").unwrap();
    let completion = transport.finish(&failed, Some("source"), None);
    assert!(completion.current && !completion.publish);
    assert_eq!(transport.displayed(), None);
    transport.seek(10);
    assert!(transport.take_request("source").is_some());
}

#[test]
fn a_changed_source_token_cannot_publish_even_without_an_epoch_reset() {
    let mut transport = Transport::default();
    transport.seek(0);
    let request = transport.take_request("source").unwrap();
    let result = transport.finish(&request, Some("different"), Some(0));
    assert!(!result.current && !result.publish);
    assert_eq!(transport.displayed(), None);
}

#[test]
fn preview_packet_preserves_actual_timestamp_and_bounded_binary_pixels() {
    let packet = [
        b"NVSFPRV1".as_slice(),
        &[33, 0, 0, 0, 0, 0, 0, 0],
        &[167, 0, 0, 0, 0, 0, 0, 0],
        b"\x89PNG\r\n\x1a\nfixture",
    ]
    .concat();
    let frame = parse_packet(&packet).unwrap();
    assert_eq!((frame.time_ms, frame.duration_ms), (33, 167));
    assert_eq!(frame.png, b"\x89PNG\r\n\x1a\nfixture");
    for invalid in [
        packet[..23].to_vec(),
        [&b"OTHER123"[..], &packet[8..]].concat(),
        [&packet[..16], &[0; 8], &packet[24..]].concat(),
        [&packet[..8], &[255; 8], &packet[16..]].concat(),
        [&packet[..24], b"not a PNG"].concat(),
        vec![0; 8 * 1024 * 1024 + 25],
    ] {
        assert!(parse_packet(&invalid).is_err());
    }
}

#[test]
fn time_jumps_accumulate_requested_positions_and_frame_steps_reanchor_them() {
    let mut transport = Transport::default();
    transport.seek(0);
    let initial = transport.take_request("source").unwrap();
    assert!(finish(&mut transport, &initial, 0));
    // Sparse source frames at 0 and 2000 ms cover several different seek targets.
    for (delta, target, displayed) in [
        (1000, 1000, 0),
        (1000, 2000, 2000),
        (1000, 3000, 2000),
        (-1000, 2000, 2000),
        (-1000, 1000, 0),
    ] {
        transport.seek(transport.position().saturating_add_signed(delta));
        let request = transport.take_request("source").unwrap();
        assert_eq!((request.at_ms, request.step), (target, 0));
        assert!(finish(&mut transport, &request, displayed));
        assert_eq!(transport.position(), target);
        assert_eq!(transport.displayed(), Some(displayed));
    }
    transport.step(1);
    let step = transport.take_request("source").unwrap();
    assert_eq!((step.at_ms, step.step), (0, 1));
    assert!(finish(&mut transport, &step, 2000));
    assert_eq!(transport.position(), 2000);
    transport.seek(transport.position().saturating_add_signed(1000));
    assert_eq!(transport.take_request("source").unwrap().at_ms, 3000);
}

#[test]
fn manual_seek_to_the_endpoint_restarts_on_the_first_play() {
    let mut transport = Transport::default();
    transport.seek(5999);
    let last = transport.take_request("source").unwrap();
    assert!(finish(&mut transport, &last, 5900));
    assert_eq!(transport.position(), 5999);
    assert_eq!(transport.displayed(), Some(5900));
    assert_eq!(transport.play(1000, 5900, 2000, 5999), 2000);
    let restarted = transport.take_request("source").unwrap();
    assert_eq!((restarted.at_ms, restarted.step), (2000, 0));
    assert!(finish(&mut transport, &restarted, 2000));
    assert_eq!(transport.tick(1250, 5999), Some((2250, false)));
}

#[test]
fn restart_preserves_in_range_start_and_rejects_start_from_a_longer_source() {
    for (start, expected) in [
        (0, 0),
        (2000, 2000),
        (5999, 5999),
        (6000, 0),
        (8000, 0),
        (u64::MAX, 0),
    ] {
        let mut transport = Transport::default();
        transport.play(0, 5500, start, 5999);
        assert_eq!(transport.tick(1000, 5999), Some((5999, true)));
        let last = transport.take_request("source").unwrap();
        assert!(finish(&mut transport, &last, 5900));
        assert_eq!(transport.play(2000, 5900, start, 5999), expected);
        let restarted = transport.take_request("source").unwrap();
        assert_eq!((restarted.at_ms, restarted.step), (expected, 0));
        assert!(restarted.at_ms <= 5999);
        assert!(finish(&mut transport, &restarted, expected));
        if start > 5999 {
            assert_eq!(transport.tick(2250, 5999), Some((250, false)));
        }
    }
}

#[test]
fn endpoint_restart_does_not_cancel_a_queued_or_active_backward_step() {
    for active in [false, true] {
        let mut transport = Transport::default();
        transport.seek(5999);
        let last = transport.take_request("source").unwrap();
        assert!(finish(&mut transport, &last, 5900));
        transport.step(-1);
        let pending = active.then(|| transport.take_request("source").unwrap());
        assert_eq!(transport.play(1000, 5900, 0, 5999), 5900);
        assert_eq!(transport.tick(1025, 5999), None);
        let step = pending.unwrap_or_else(|| transport.take_request("source").unwrap());
        assert_eq!((step.at_ms, step.step), (5900, -1));
        assert!(finish(&mut transport, &step, 5800));
        assert_eq!(transport.position(), 5800);
        assert_eq!(transport.tick(1050, 5999), Some((5850, false)));
    }
}
