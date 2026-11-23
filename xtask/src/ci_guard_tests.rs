use super::{Guard, Result, hosted, run};
use std::{
    env,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const FIXTURE: &str = "__ci-guard-fixture";

fn fixture_command(case: &str) -> io::Result<Command> {
    let mut command = Command::new(env::current_exe()?);
    command.args([FIXTURE, case]);
    Ok(command)
}

#[allow(
    clippy::zombie_processes,
    reason = "The leak fixture deliberately leaves one owned descendant for the guard to reject and reap."
)]
pub(super) fn fixture(case: &str) -> Result<i32> {
    hosted("Guard regressions run only on GitHub-hosted Actions runners.")?;
    let root = super::root();
    let cwd = env::current_dir()?;
    let state = root.join(".ci-work");
    if cwd.parent() != Some(state.as_path())
        || !cwd
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("guard-contract-"))
    {
        return Err("Guard fixtures require an owned contract directory.".into());
    }
    match case {
        "unicode" => println!("guard 日本 فارسی"),
        "failure" => return Ok(7),
        "leak" | "retained" => {
            let mut child = fixture_command(if case == "leak" { "sleep" } else { "held" })?
                .stdin(Stdio::null())
                .spawn()?;
            fs::write("child.tmp", child.id().to_string())?;
            fs::rename("child.tmp", "child.pid")?;
            if case == "retained" && !child.wait()?.success() {
                return Err("Retained fixture child did not exit successfully.".into());
            }
        }
        "held" => {
            let started = Instant::now();
            while !Path::new("held.open").is_file() {
                if started.elapsed() >= Duration::from_secs(8) {
                    return Err("Retained-handle fixture was not opened.".into());
                }
                thread::sleep(super::POLL);
            }
        }
        "file-progress" => {
            fs::create_dir_all("buildlogs/ffmpeg/source/deep")?;
            for i in 0..1000 {
                fs::write(format!("buildlogs/ffmpeg/source/deep/unit-{i}.c"), "noise")?;
            }
            let mut log = OpenOptions::new()
                .create(true)
                .append(true)
                .open("buildlogs/ffmpeg/build.log")?;
            for i in 0..12 {
                writeln!(log, "completed-unit={i}")?;
                thread::sleep(Duration::from_millis(500));
            }
        }
        "frozen-progress" => {
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(12) {
                for file in [
                    "buildlogs/ffmpeg/source/deep/noise.log",
                    "buildlogs/unit-0.c",
                    "buildlogs/ffmpeg/unit-0.c",
                ] {
                    writeln!(
                        OpenOptions::new().create(true).append(true).open(file)?,
                        "noise"
                    )?;
                }
                thread::sleep(super::POLL);
            }
        }
        "stalled-observer" | "wall" => {
            for i in 0..12 {
                println!("stream-progress={i}");
                thread::sleep(Duration::from_millis(500));
            }
            thread::sleep(Duration::from_secs(60));
        }
        "sleep" => thread::sleep(Duration::from_secs(60)),
        _ => return Err("Unknown guard contract fixture.".into()),
    }
    Ok(0)
}

fn require(condition: bool, error: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(error.to_owned().into())
    }
}

fn invoke(
    work: &Path,
    case: &str,
    payload: &str,
    progress: Option<&Path>,
    stall: bool,
) -> Result<(i32, PathBuf)> {
    let capture = work.join(format!("{case}.stdout"));
    let name = format!("contract-{case}");
    let mut command = fixture_command(payload)?;
    command.current_dir(work);
    if case == "unicode" {
        // A bare executable name with duplicate PATH candidates must still resolve to one program.
        let exe = env::current_exe()?;
        let bin = exe.parent().ok_or("Missing fixture executable parent.")?;
        let mut path = vec![bin.to_path_buf(), bin.to_path_buf()];
        if let Some(inherited) = env::var_os("PATH") {
            path.extend(env::split_paths(&inherited));
        }
        command = Command::new(exe.file_name().ok_or("Missing fixture executable name.")?);
        command
            .args([FIXTURE, payload])
            .current_dir(work)
            .env("PATH", env::join_paths(path)?);
    }
    let options = Guard {
        name: &name,
        wall_s: if case == "wall" { 2 } else { 15 },
        idle_s: 5,
        capture: Some(&capture),
        progress,
        stall,
    };
    let outcome = run(options, command)?;
    Ok((outcome.code, outcome.log))
}

pub(super) fn contracts() -> Result<()> {
    hosted("Guard regressions run only on GitHub-hosted Actions runners.")?;
    let root = super::root();
    let state = super::state_dir(&root)?;
    let work = tempfile::Builder::new()
        .prefix("guard-contract-")
        .tempdir_in(&state)?;
    let work = work.path();
    require(
        invoke(work, "unicode", "unicode", None, false)?.0 == 0,
        "Unicode fixture failed.",
    )?;
    require(
        fs::read_to_string(work.join("unicode.stdout"))?.trim() == "guard 日本 فارسی",
        "Guard did not preserve exact Unicode stdout.",
    )?;
    #[cfg(unix)]
    super::ownership::absent_group_contract()?;
    require(
        invoke(work, "failure", "failure", None, false)?.0 == 7,
        "Guard did not preserve exit code 7.",
    )?;
    require(
        invoke(work, "leak", "leak", None, false)?.0 == 1,
        "Guard accepted an actual surviving descendant.",
    )?;
    let pid = fs::read_to_string(work.join("child.pid"))?.parse()?;
    require(
        !super::ownership::pid_alive(pid)?,
        "Guard left its real regression child alive.",
    )?;
    #[cfg(windows)]
    {
        retained(work)?;
        short_lived(work)?;
    }

    let progress = work.join("buildlogs");
    for port in 0..43 {
        let dir = progress.join(format!("port-{port}"));
        fs::create_dir_all(&dir)?;
        for stage in 0..4 {
            fs::write(dir.join(format!("stage-{stage}.log")), "owned baseline")?;
        }
    }
    fs::create_dir(progress.join("ffmpeg"))?;
    for entry in 0..128 {
        for directory in [progress.clone(), progress.join("ffmpeg")] {
            fs::write(directory.join(format!("unit-{entry}.c")), "noise")?;
        }
    }
    verify_observer(work, "file-progress", "file-progress", &progress, false, 0)?;
    require(
        fs::read_to_string(progress.join("ffmpeg/build.log"))?
            .lines()
            .count()
            == 12,
        "Quiet build missed actual writes.",
    )?;
    let assembly = super::observer::assembly_identity()?;
    verify_observer(
        work,
        "frozen-progress",
        "frozen-progress",
        &progress,
        false,
        124,
    )?;
    require(
        super::observer::assembly_identity()? == assembly,
        "Guard replaced its source-bound observer assembly.",
    )?;
    let started = Instant::now();
    verify_observer(
        work,
        "stalled-observer",
        "stalled-observer",
        &progress,
        true,
        124,
    )?;
    require(
        started.elapsed() < Duration::from_secs(20)
            && fs::read_to_string(work.join("stalled-observer.stdout"))?
                .lines()
                .count()
                == 12,
        "Stalled observer blocked stream progress, deadlines or cleanup.",
    )?;
    let invalid = work.join("invalid-buildlogs");
    fs::write(&invalid, "a regular file cannot be a log directory")?;
    verify_observer(work, "observer-error", "sleep", &invalid, false, 1)?;
    let (code, log) = invoke(work, "wall", "wall", None, false)?;
    require(
        code == 124 && fs::read_to_string(log)?.contains("Wall timeout"),
        "Wall deadline did not override stream progress.",
    )?;
    let cases = if cfg!(windows) { 11 } else { 9 };
    println!(
        "Guard contracts passed: tests={cases} failed=0 skipped=0; Unicode, exit status, child cleanup, quiet/frozen/noisy logs, stalled/error observer ownership and platform process-handle semantics."
    );
    Ok(())
}

#[cfg(windows)]
fn short_lived(work: &Path) -> Result<()> {
    let started = Instant::now();
    for (index, (payload, expected)) in [
        ("unicode", 0),
        ("failure", 7),
        ("unicode", 0),
        ("failure", 7),
    ]
    .into_iter()
    .enumerate()
    {
        let remaining = Duration::from_secs(8).saturating_sub(started.elapsed());
        require(
            remaining >= Duration::from_secs(1),
            "Short-lived owned command sequence exceeded its total bound.",
        )?;
        let name = format!("contract-short-lived-{index}");
        let capture = work.join(format!("{name}.stdout"));
        let mut command = fixture_command(payload)?;
        command.current_dir(work);
        let outcome = run(
            Guard {
                name: &name,
                wall_s: remaining.as_secs().min(2) as u32,
                idle_s: 1,
                capture: Some(&capture),
                progress: None,
                stall: false,
            },
            command,
        )?;
        require(
            outcome.code == expected
                && fs::read_to_string(outcome.log)?.contains("Cleanup verified pid="),
            "Short-lived owned command lost its exit status or verified cleanup.",
        )?;
        require(
            fs::read_to_string(capture)?.trim()
                == if expected == 0 {
                    "guard 日本 فارسی"
                } else {
                    ""
                },
            "Short-lived owned command lost its final stdout.",
        )?;
    }
    require(
        started.elapsed() < Duration::from_secs(8),
        "Short-lived owned command sequence exceeded its total bound.",
    )
}

fn verify_observer(
    work: &Path,
    case: &str,
    payload: &str,
    dir: &Path,
    stall: bool,
    expected: i32,
) -> Result<()> {
    let (code, log) = invoke(work, case, payload, Some(dir), stall)?;
    let text = fs::read_to_string(log)?;
    require(
        code == expected,
        &format!("Unexpected status {code} for {case}."),
    )?;
    require(
        text.contains("Observer cleanup verified pid=")
            && (case == "observer-error" || text.contains(" Cleanup verified pid=")),
        "Observer or payload cleanup was not verified.",
    )?;
    require(
        expected != 124 || text.contains("Idle timeout"),
        "Expected idle timeout diagnostic.",
    )
}

#[cfg(windows)]
fn retained(work: &Path) -> Result<()> {
    let pid_file = work.join("child.pid");
    fs::remove_file(&pid_file)?;
    let marker = work.join("held.open");
    thread::scope(|scope| -> Result<()> {
        let watcher = scope.spawn(move || {
            let observe = || -> Result<bool> {
                let started = Instant::now();
                while !pid_file.is_file() {
                    if started.elapsed() >= Duration::from_secs(8) {
                        return Err("Retained-handle child did not appear.".into());
                    }
                    thread::sleep(super::POLL);
                }
                let held =
                    super::ownership::retained_handle(fs::read_to_string(pid_file)?.parse()?)?;
                fs::write(marker, "opened")?;
                let exited = held.wait(Duration::from_secs(8))?;
                while !work.join("held.done").is_file()
                    && started.elapsed() < Duration::from_secs(14)
                {
                    thread::sleep(super::POLL);
                }
                Ok(exited)
            };
            observe().map_err(|error| error.to_string())
        });
        let outcome = invoke(work, "retained-exited-handle", "retained", None, false);
        fs::write(work.join("held.done"), "finished")?;
        let exited = watcher
            .join()
            .map_err(|_| "Retained-handle watcher panicked.")??;
        require(
            outcome?.0 == 0 && exited,
            "Guard rejected a retained handle to an exited child.",
        )
    })
}
