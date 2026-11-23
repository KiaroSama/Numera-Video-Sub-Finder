use serde_json::{Value, json};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::atomic::{AtomicU32, Ordering},
    time::{Duration, Instant},
};

static NEXT: AtomicU32 = AtomicU32::new(0);

fn root() -> PathBuf {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("nvsf-parity-{}-{id}", std::process::id()));
    std::fs::create_dir(&path).expect("create test root");
    std::fs::create_dir(path.join("data")).expect("create data dir");
    path
}

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(Some(_))) {
            return;
        }
        let _ = self.0.kill();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if matches!(self.0.try_wait(), Ok(Some(_))) {
                return;
            }
            if Instant::now() >= deadline {
                panic!("CLI child could not be reaped within two seconds");
            }
            std::thread::park_timeout(Duration::from_millis(10));
        }
    }
}

fn nvsf(root: &Path, arguments: &[&str]) -> Output {
    let stdout = root.join("stdout.txt");
    let stderr = root.join("stderr.txt");
    let mut command = Command::new(env!("CARGO_BIN_EXE_nvsf"));
    command
        .arg("--data-root")
        .arg(root.join("data"))
        .args(arguments)
        .current_dir(root)
        .env("NVSF_RUNTIME_ROOT", root.join("empty-runtimes"))
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&stdout).expect("stdout file"))
        .stderr(std::fs::File::create(&stderr).expect("stderr file"));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = OwnedChild(command.spawn().expect("spawn nvsf"));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.0.try_wait().expect("try_wait") {
            break status;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "CLI exceeded ten-second wall limit"
        );
        std::thread::park_timeout(Duration::from_millis(10));
    };
    let capture = |path: &Path| {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .expect("open capture")
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .expect("read capture");
        bytes
    };
    Output {
        status,
        stdout: capture(&stdout),
        stderr: capture(&stderr),
    }
}

fn json_stdout(output: &Output) -> Value {
    let text = std::str::from_utf8(&output.stdout).expect("utf-8 stdout");
    serde_json::from_str(text).expect("json stdout")
}

fn write_sample_project(path: &Path) {
    let project = json!({
        "schema": 1,
        "source_hash": "a".repeat(64),
        "config_hash": "b".repeat(64),
        "model_hash": "c".repeat(64),
        "config": {
            "input": "sample.mp4",
            "output": "sample.srt",
            "data_root": null,
            "language": "en",
            "device": "auto",
            "quality": "balanced",
            "regions": [{"x": 0.0, "y": 0.72, "width": 1.0, "height": 0.25}],
            "exclusions": [],
            "start_ms": 0,
            "end_ms": null,
            "threads": 2,
            "overwrite": false,
            "normalize_persian": false,
            "min_confidence": 0.7,
            "gpu_required": false,
            "accelerator_required": false
        },
        "media": {
            "width": 1920,
            "height": 1080,
            "duration_ms": 12000,
            "codec": "h264",
            "time_base_num": 1,
            "time_base_den": 1000,
            "subtitle_streams": 0,
            "rotation": 0
        },
        "cues": [
            {
                "id": 1,
                "track": 0,
                "start_ms": 1000,
                "end_ms": 4000,
                "text": "Opening line",
                "raw_text": "Opening line",
                "confidence": 0.94,
                "reviewed": true,
                "edited": false,
                "crop": null
            },
            {
                "id": 2,
                "track": 0,
                "start_ms": 4500,
                "end_ms": 8000,
                "text": "Uncertain line",
                "raw_text": "Uncertain line",
                "confidence": 0.42,
                "reviewed": false,
                "edited": false,
                "crop": null
            }
        ],
        "processed_ms": 12000,
        "active_appearances": [],
        "completed": true,
        "warnings": []
    });
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&project).expect("serialize"),
    )
    .expect("write");
}

#[test]
fn project_inspect_search_edit_accept_revert_split_merge_and_save() {
    let dir = root();
    let proj = dir.join("sample.nvsf.json");
    let proj_str = proj.to_str().expect("utf8 path");
    write_sample_project(&proj);

    let out = nvsf(&dir, &["project", "inspect", proj_str]);
    assert_eq!(out.status.code(), Some(0));
    let v = json_stdout(&out);
    assert_eq!(v["counts"]["total"], 2);
    assert_eq!(v["counts"]["uncertain"], 1);
    assert_eq!(v["review_needed"], true);

    let out = nvsf(
        &dir,
        &[
            "project",
            "cues",
            proj_str,
            "--filter",
            "uncertain",
            "--query",
            "uncertain",
        ],
    );
    assert_eq!(out.status.code(), Some(0));
    let v = json_stdout(&out);
    assert_eq!(v["matched"], 1);
    assert_eq!(v["cues"][0]["id"], 2);

    let out = nvsf(
        &dir,
        &[
            "project",
            "edit-cue",
            proj_str,
            "--id",
            "2",
            "--text",
            "Fixed subtitle",
            "--start-ms",
            "4600",
        ],
    );
    assert_eq!(out.status.code(), Some(0));
    let v = json_stdout(&out);
    assert_eq!(v["cue"]["text"], "Fixed subtitle");
    assert_eq!(v["cue"]["edited"], true);

    let out = nvsf(&dir, &["project", "accept-cue", proj_str, "--id", "2"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json_stdout(&out)["cue"]["reviewed"], true);

    let out = nvsf(&dir, &["project", "revert-cue", proj_str, "--id", "2"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json_stdout(&out)["cue"]["text"], "Uncertain line");

    let out = nvsf(
        &dir,
        &[
            "project",
            "split-cue",
            proj_str,
            "--id",
            "2",
            "--at-ms",
            "6000",
        ],
    );
    assert_eq!(out.status.code(), Some(0));
    let split = json_stdout(&out);
    assert_eq!(split["total_cues"], 3);
    let second_id = split["cues"][1]["id"]
        .as_u64()
        .expect("split id")
        .to_string();

    let out = nvsf(
        &dir,
        &[
            "project",
            "merge-cues",
            proj_str,
            "--first",
            "2",
            "--second",
            &second_id,
        ],
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json_stdout(&out)["total_cues"], 2);

    let copy = dir.join("saved.nvsf.json");
    let out = nvsf(
        &dir,
        &[
            "project",
            "save",
            proj_str,
            "--output",
            copy.to_str().expect("copy path"),
        ],
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(copy.is_file());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn project_merge_requires_review_and_revert_restores_both_original_subtitles() {
    let dir = root();
    let proj = dir.join("merged.nvsf.json");
    let proj_str = proj.to_str().expect("utf8 path");
    write_sample_project(&proj);

    let out = nvsf(
        &dir,
        &[
            "project",
            "merge-cues",
            proj_str,
            "--first",
            "1",
            "--second",
            "2",
        ],
    );
    assert_eq!(out.status.code(), Some(0));
    let merged = json_stdout(&out);
    assert_eq!(merged["cue"]["text"], "Opening line\nUncertain line");
    assert_eq!(merged["cue"]["raw_text"], "Opening line\nUncertain line");
    assert_eq!(merged["cue"]["reviewed"], false);

    let out = nvsf(&dir, &["project", "inspect", proj_str]);
    assert_eq!(out.status.code(), Some(0));
    let inspected = json_stdout(&out);
    assert_eq!(inspected["counts"]["total"], 1);
    assert_eq!(inspected["counts"]["unreviewed"], 1);
    assert_eq!(inspected["review_needed"], true);

    let out = nvsf(&dir, &["project", "revert-cue", proj_str, "--id", "1"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        json_stdout(&out)["cue"]["text"],
        "Opening line\nUncertain line"
    );

    std::fs::remove_dir_all(dir).expect("remove fixtures");
}

#[test]
fn queue_and_preferences_and_fail_closed_gates_work_end_to_end() {
    let dir = root();
    let clip = dir.join("clip.mp4");
    std::fs::write(&clip, b"video bytes").expect("write clip");
    let out_srt = dir.join("clip.srt");

    let out = nvsf(&dir, &["queue", "list"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json_stdout(&out)["count"], 0);

    let out = nvsf(
        &dir,
        &[
            "queue",
            "add",
            "--input",
            clip.to_str().expect("clip"),
            "--output",
            out_srt.to_str().expect("out"),
        ],
    );
    assert_eq!(out.status.code(), Some(0));
    let added = json_stdout(&out);
    assert_eq!(added["count"], 1);
    let id = added["added"].as_str().expect("id").to_owned();

    // Verify desktop-queue.json conforms to schema 1.
    let raw: Value = serde_json::from_slice(
        &std::fs::read(dir.join("data").join("desktop-queue.json")).expect("read queue"),
    )
    .expect("parse queue");
    assert_eq!(raw["schema"], 1);
    assert_eq!(raw["records"].as_array().expect("records").len(), 1);

    let out = nvsf(&dir, &["queue", "remove", "--id", &id]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json_stdout(&out)["count"], 0);

    let out = nvsf(&dir, &["preferences", "get"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json_stdout(&out)["update_checks_enabled"], true);

    let out = nvsf(&dir, &["preferences", "set", "--update-checks", "false"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json_stdout(&out)["update_checks_enabled"], false);

    let out = nvsf(&dir, &["updates", "status"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json_stdout(&out)["enabled"], false);

    let out = nvsf(&dir, &["updates", "config", "--enabled", "true"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json_stdout(&out)["enabled"], true);

    // Fail-closed gates must return exit code 3.
    let out = nvsf(&dir, &["updates", "download", "--token", "missing"]);
    assert_eq!(out.status.code(), Some(3));
    let out = nvsf(&dir, &["project", "visual-retry-install"]);
    assert_eq!(out.status.code(), Some(3));
    let out = nvsf(&dir, &["bulk", "--concurrency", "9", "--input", "a.mp4"]);
    assert_eq!(out.status.code(), Some(3));
    let out = nvsf(
        &dir,
        &[
            "preview",
            "strip",
            "--input",
            "a.mp4",
            "--output-dir",
            "frames",
            "--count",
            "25",
        ],
    );
    assert_eq!(out.status.code(), Some(3));
    let out = nvsf(
        &dir,
        &[
            "preview",
            "frame",
            "--input",
            "a.mp4",
            "--at-ms",
            "0",
            "--output",
            "f.png",
            "--pixel-box",
            "10,10,0,50",
        ],
    );
    assert_eq!(out.status.code(), Some(3));

    let _ = std::fs::remove_dir_all(&dir);
}
