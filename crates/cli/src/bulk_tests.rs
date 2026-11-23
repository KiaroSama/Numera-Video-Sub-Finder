use super::*;
use nvsf_core::{Cue, MediaInfo};
use std::sync::atomic::AtomicU32;

static SEQ: AtomicU32 = AtomicU32::new(0);

fn temp_dir() -> PathBuf {
    let id = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("nvsf-bulk-test-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn fake_project(cfg: &ExtractConfig, reviewed: bool) -> Project {
    let mut cue = Cue::new(1, 0, 0, 500, "Test".into()).expect("cue");
    cue.reviewed = reviewed;
    cue.confidence = Some(if reviewed { 0.95 } else { 0.3 });
    Project {
        schema: 1,
        source_hash: "a".repeat(64),
        config_hash: "b".repeat(64),
        model_hash: "c".repeat(64),
        config: cfg.clone(),
        media: MediaInfo {
            width: 640,
            height: 360,
            duration_ms: Some(500),
            codec: "h264".into(),
            time_base_num: 1,
            time_base_den: 1000,
            subtitle_streams: 0,
            rotation: 0,
        },
        cues: vec![cue],
        processed_ms: 500,
        active_appearances: vec![],
        completed: true,
        warnings: vec![],
    }
}

#[test]
fn parses_json_and_newline_inputs_files() {
    let json_list = parse_inputs_text(r#"["a.mp4", "b.mkv"]"#).expect("json");
    assert_eq!(
        json_list,
        vec![PathBuf::from("a.mp4"), PathBuf::from("b.mkv")]
    );

    let lines = parse_inputs_text("# comment\n\n clip1.mp4 \nclip2.webm\n").expect("lines");
    assert_eq!(
        lines,
        vec![PathBuf::from("clip1.mp4"), PathBuf::from("clip2.webm")]
    );
}

#[test]
fn input_dir_filters_extensions_and_resolves_output_collisions() {
    let dir = temp_dir();
    let out = dir.join("out");
    std::fs::create_dir_all(&out).expect("out dir");
    std::fs::write(dir.join("a.mp4"), b"1").expect("a");
    std::fs::write(dir.join("a.mkv"), b"2").expect("b");
    std::fs::write(dir.join("notes.txt"), b"3").expect("txt");

    let args = BulkArgs {
        inputs: vec![],
        inputs_file: None,
        input_dir: Some(dir.clone()),
        extensions: "mp4,mkv".into(),
        output_dir: Some(out.clone()),
        concurrency: 2,
        fail_fast: false,
        language: "en".into(),
        device: "auto".into(),
        quality: "balanced".into(),
        roi: vec![],
        exclude: vec![],
        start_ms: 0,
        end_ms: None,
        threads: 4,
        format: "srt".into(),
        overwrite: false,
        resume: false,
        bom: false,
        crlf: false,
        normalize_persian: false,
        min_confidence: 0.7,
        gpu_required: false,
        accelerator_required: false,
    };

    let discovered = discover_inputs(&args).expect("discover");
    assert_eq!(discovered.len(), 2);
    let planned = plan_outputs(&discovered, Some(&out), "srt").expect("plan");
    assert_eq!(planned.len(), 2);
    assert_ne!(planned[0], planned[1]);
    assert_eq!(per_job_threads(8, 2), 4);
    assert_eq!(per_job_threads(2, 4), 1);
    assert!(validate_concurrency(0).is_err());
    assert!(validate_concurrency(9).is_err());
    assert!(validate_concurrency(4).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parallel_pool_isolates_failures_and_computes_deterministic_exit_codes() {
    let jobs: Vec<_> = (1..=4)
        .map(|i| {
            let input = PathBuf::from(format!("v{i}.mp4"));
            let output = PathBuf::from(format!("v{i}.srt"));
            let cfg = ExtractConfig {
                input: format!("v{i}.mp4"),
                output: format!("v{i}.srt"),
                ..Default::default()
            };
            (input, output, cfg)
        })
        .collect();

    let control = Control::default();
    let results = execute_pool(jobs.clone(), 3, false, &control, &|cfg, _| {
        if cfg.input == "v2.mp4" {
            Err(nvsf_app::fail("simulated decode failure"))
        } else if cfg.input == "v3.mp4" {
            Ok(fake_project(&cfg, false))
        } else {
            Ok(fake_project(&cfg, true))
        }
    });
    assert_eq!(results.len(), 4);
    assert_eq!(results[0].status, "ok");
    assert_eq!(results[1].status, "failed");
    assert_eq!(results[2].status, "review_needed");
    assert_eq!(results[3].status, "ok");
    assert_eq!(summarize_exit_code(&results), 4);

    let ok_review = execute_pool(jobs[0..3].to_vec(), 2, false, &control, &|cfg, _| {
        Ok(fake_project(&cfg, cfg.input != "v3.mp4"))
    });
    assert_eq!(summarize_exit_code(&ok_review), 2);
}

#[test]
fn fail_fast_cancels_remaining_jobs_on_first_failure() {
    let jobs: Vec<_> = (1..=3)
        .map(|i| {
            let input = PathBuf::from(format!("f{i}.mp4"));
            let output = PathBuf::from(format!("f{i}.srt"));
            let cfg = ExtractConfig {
                input: format!("f{i}.mp4"),
                output: format!("f{i}.srt"),
                ..Default::default()
            };
            (input, output, cfg)
        })
        .collect();

    let control = Control::default();
    let results = execute_pool(jobs, 1, true, &control, &|cfg, _| {
        if cfg.input == "f1.mp4" {
            Err(nvsf_app::fail("boom"))
        } else {
            Ok(fake_project(&cfg, true))
        }
    });
    assert_eq!(results[0].status, "failed");
    assert_eq!(results[1].status, "cancelled");
    assert_eq!(results[2].status, "cancelled");
    assert_eq!(summarize_exit_code(&results), 4);
}

#[test]
fn fail_fast_running_job_observes_user_cancellation() {
    let cfg = ExtractConfig {
        input: "cancel.mp4".into(),
        output: "cancel.srt".into(),
        ..Default::default()
    };
    let jobs = vec![("cancel.mp4".into(), "cancel.srt".into(), cfg)];
    let control = Control::default();
    let results = execute_pool(jobs, 1, true, &control, &|cfg, job_control| {
        // The CLI signal handler sets this token after extraction has started.
        control.cancel.store(true, Ordering::Release);
        if job_control.cancel.load(Ordering::Acquire) {
            Err(AppError::Cancelled)
        } else {
            Ok(fake_project(&cfg, true))
        }
    });
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, "cancelled");
    assert_eq!(results[0].exit_code, 5);
    assert_eq!(summarize_exit_code(&results), 5);
}

#[test]
fn fail_fast_failure_cancels_an_already_running_peer() {
    let jobs: Vec<_> = (1..=2)
        .map(|i| {
            let input = PathBuf::from(format!("peer{i}.mp4"));
            let output = PathBuf::from(format!("peer{i}.srt"));
            let cfg = ExtractConfig {
                input: format!("peer{i}.mp4"),
                output: format!("peer{i}.srt"),
                ..Default::default()
            };
            (input, output, cfg)
        })
        .collect();
    let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
    let started_rx = Mutex::new(started_rx);
    let control = Control::default();
    let results = execute_pool(jobs, 2, true, &control, &|cfg, job_control| {
        if cfg.input == "peer1.mp4" {
            started_rx
                .lock()
                .expect("peer startup receiver")
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("second extraction started");
            return Err(nvsf_app::fail("first extraction failed"));
        }
        started_tx.send(()).expect("report peer startup");
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        while !job_control.cancel.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "running peer was not cancelled");
            std::thread::park_timeout(std::time::Duration::from_millis(1));
        }
        Err(AppError::Cancelled)
    });
    assert_eq!(results[0].status, "failed");
    assert_eq!(results[0].exit_code, 4);
    assert_eq!(results[1].status, "cancelled");
    assert_eq!(results[1].exit_code, 5);
    assert_eq!(summarize_exit_code(&results), 4);
}

#[test]
fn bulk_srt_honors_each_requested_encoding() {
    use clap::Parser;
    let dir = temp_dir();
    for (index, flags, expected) in [
        (0, vec![], "1\n00:00:00,000 --> 00:00:00,500\nTest\n\n"),
        (
            1,
            vec!["--bom"],
            "\u{feff}1\n00:00:00,000 --> 00:00:00,500\nTest\n\n",
        ),
        (
            2,
            vec!["--crlf"],
            "1\r\n00:00:00,000 --> 00:00:00,500\r\nTest\r\n\r\n",
        ),
        (
            3,
            vec!["--bom", "--crlf"],
            "\u{feff}1\r\n00:00:00,000 --> 00:00:00,500\r\nTest\r\n\r\n",
        ),
    ] {
        let input = dir.join(format!("encoding-{index}.mp4"));
        std::fs::write(&input, b"controlled extraction input").expect("input fixture");
        let mut argv = vec!["nvsf", "bulk", "--input", input.to_str().unwrap()];
        argv.extend(flags);
        let crate::Commands::Bulk(args) = crate::Cli::try_parse_from(argv).unwrap().command else {
            panic!("bulk command expected");
        };
        let status = run_bulk_with(*args, None, &Control::default(), &|cfg, _, options| {
            let project = fake_project(&cfg, true);
            nvsf_app::export_project(
                &project,
                Path::new(&cfg.output),
                "srt",
                cfg.overwrite,
                options.bom,
                options.crlf,
            )?;
            Ok(project)
        })
        .expect("bulk export");
        assert_eq!(status, 0);
        assert_eq!(
            std::fs::read(input.with_extension("srt")).expect("SRT bytes"),
            expected.as_bytes()
        );
    }
    std::fs::remove_dir_all(dir).expect("remove fixtures");
}

#[test]
fn bulk_srt_encoding_flags_do_not_authorize_replacing_existing_output() {
    use clap::Parser;
    let dir = temp_dir();
    let input = dir.join("existing.mp4");
    let output = input.with_extension("srt");
    std::fs::write(&input, b"controlled extraction input").expect("input fixture");
    std::fs::write(&output, b"Existing corrected subtitles").expect("existing output");
    let crate::Commands::Bulk(args) = crate::Cli::try_parse_from([
        "nvsf",
        "bulk",
        "--input",
        input.to_str().unwrap(),
        "--bom",
        "--crlf",
    ])
    .unwrap()
    .command
    else {
        panic!("bulk command expected");
    };
    let status = run_bulk_with(*args, None, &Control::default(), &|cfg, _, options| {
        let project = fake_project(&cfg, true);
        nvsf_app::export_project(
            &project,
            Path::new(&cfg.output),
            "srt",
            cfg.overwrite,
            options.bom,
            options.crlf,
        )?;
        Ok(project)
    })
    .expect("bulk result");
    assert_eq!(status, 4);
    assert_eq!(
        std::fs::read(output).expect("preserved output"),
        b"Existing corrected subtitles"
    );
    std::fs::remove_dir_all(dir).expect("remove fixtures");
}

#[test]
fn bulk_srt_never_replaces_an_edit_after_the_workers_first_publication() {
    use clap::Parser;
    let dir = temp_dir();
    let input = dir.join("concurrent-edit.mp4");
    let output = input.with_extension("srt");
    std::fs::write(&input, b"controlled extraction input").expect("input fixture");
    let crate::Commands::Bulk(args) = crate::Cli::try_parse_from([
        "nvsf",
        "bulk",
        "--input",
        input.to_str().unwrap(),
        "--bom",
        "--crlf",
    ])
    .unwrap()
    .command
    else {
        panic!("bulk command expected");
    };
    let status = run_bulk_with(*args, None, &Control::default(), &|cfg, _, options| {
        assert!(
            !cfg.overwrite,
            "encoding must preserve the user's overwrite policy"
        );
        let project = fake_project(&cfg, true);
        let destination = Path::new(&cfg.output);
        nvsf_app::export_project(
            &project,
            destination,
            "srt",
            cfg.overwrite,
            options.bom,
            options.crlf,
        )?;
        assert_eq!(
            std::fs::read(destination)?,
            b"\xef\xbb\xbf1\r\n00:00:00,000 --> 00:00:00,500\r\nTest\r\n\r\n"
        );
        // A user edit after the worker releases output ownership must survive bulk completion.
        std::fs::write(destination, b"Concurrent corrected subtitle text")?;
        Ok(project)
    })
    .expect("bulk result");
    assert_eq!(status, 0);
    assert_eq!(
        std::fs::read(output).expect("user edit"),
        b"Concurrent corrected subtitle text"
    );
    std::fs::remove_dir_all(dir).expect("remove fixtures");
}

#[test]
fn bulk_validation_extensions_inputs_planning_and_exit_codes() {
    assert!(validate_concurrency(0).is_err());
    assert!(validate_concurrency(1).is_ok());
    assert!(validate_concurrency(8).is_ok());
    assert!(validate_concurrency(9).is_err());
    assert_eq!(per_job_threads(8, 2), 4);
    assert_eq!(per_job_threads(1, 4), 1);

    let exts = parse_extensions(".MP4, mkv , mov").expect("exts");
    assert_eq!(exts, vec!["mp4", "mkv", "mov"]);
    assert!(parse_extensions("   ,  ").is_err());

    let json_list = parse_inputs_text(r#"["a.mp4", "b.mkv"]"#).expect("json inputs");
    assert_eq!(json_list.len(), 2);
    let line_list = parse_inputs_text("# comment\n\nfirst.mp4\nsecond.mkv\n").expect("lines");
    assert_eq!(line_list.len(), 2);
    assert!(parse_inputs_text("[not valid json").is_err());

    let dir = temp_dir();
    let sub_a = dir.join("a");
    let sub_b = dir.join("b");
    let out_dir = dir.join("out");
    std::fs::create_dir_all(&sub_a).expect("sub_a");
    std::fs::create_dir_all(&sub_b).expect("sub_b");
    std::fs::create_dir_all(&out_dir).expect("out_dir");
    let f1 = sub_a.join("clip.mp4");
    let f2 = sub_b.join("clip.mp4");
    std::fs::write(&f1, b"1").expect("f1");
    std::fs::write(&f2, b"2").expect("f2");

    let planned = plan_outputs(&[f1.clone(), f2.clone()], Some(&out_dir), "vtt").expect("plan");
    assert_eq!(planned.len(), 2);
    assert_ne!(planned[0], planned[1]);
    assert!(planned[0].ends_with("clip.vtt"));
    assert!(planned[1].ends_with("clip-2.vtt"));

    let side_by_side = plan_outputs(&[f1.clone()], None, "srt").expect("side by side");
    assert_eq!(side_by_side[0], sub_a.join("clip.srt"));
    assert!(plan_outputs(&[f1], Some(&dir.join("missing-out")), "srt").is_err());

    let mk = |status: &str, code: i32, review: bool| BulkItemResult {
        input: "a.mp4".into(),
        output: "a.srt".into(),
        status: status.into(),
        exit_code: code,
        cues: 1,
        review_needed: review,
        elapsed_ms: 1,
        error: None,
    };
    assert_eq!(summarize_exit_code(&[mk("ok", 0, false)]), 0);
    assert_eq!(summarize_exit_code(&[mk("review_needed", 2, true)]), 2);
    assert_eq!(summarize_exit_code(&[mk("cancelled", 5, false)]), 5);
    assert_eq!(summarize_exit_code(&[mk("failed", 6, false)]), 6);

    let _ = std::fs::remove_dir_all(&dir);
}
