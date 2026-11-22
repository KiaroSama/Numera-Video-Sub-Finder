use super::{FILE_LIMIT, Result, cmd};
use std::{
    io::{self, Read, Write},
    path::Path,
    process::Stdio,
    thread,
    time::{Duration, Instant},
};

fn exact_symbol(data: &[u8]) -> usize {
    data.split(|byte| *byte == b'\n')
        .filter(|line| *line == b"__asan_init")
        .count()
}

fn nm_output(mut stream: impl Read) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    (&mut stream).take(FILE_LIMIT).read_to_end(&mut output)?;
    if output.len() as u64 >= FILE_LIMIT {
        return Err(io::Error::other(
            "Actual caller symbol inspection exceeded its output bound.",
        ));
    }
    Ok(output)
}

pub(super) fn inspect_caller(path: &Path) -> Result<()> {
    for (data, expected) in [
        (b"__asan_init\n".as_slice(), 1),
        (b"other\n__asan_init\n", 1),
        (b"", 0),
        (b"prefix__asan_init\n__asan_init_suffix\n", 0),
        (b"__asan_init\n__asan_init\n", 2),
    ] {
        if exact_symbol(data) != expected {
            return Err("Exact symbol reader regression failed.".into());
        }
    }
    let mut child = cmd(
        "nm",
        &["-D", "--defined-only", "--no-sort", "--format=just-symbols"],
    )
    .arg(path)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()?;
    println!("Inspecting defined dynamic symbols pid={}", child.id());
    io::stdout().flush()?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Missing caller symbol output pipe.")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("Missing caller symbol diagnostic pipe.")?;
    let output = thread::spawn(move || nm_output(stdout));
    let diagnostics = thread::spawn(move || nm_output(stderr));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= Duration::from_secs(3) {
            let _ = child.kill();
            return Err("Actual caller symbol inspection exceeded three seconds.".into());
        }
        thread::sleep(Duration::from_millis(20));
    };
    let output = output
        .join()
        .map_err(|_| "Caller symbol reader panicked.")??;
    diagnostics
        .join()
        .map_err(|_| "Caller symbol diagnostic reader panicked.")??;
    if !status.success() {
        return Err("Actual caller symbol inspection failed.".into());
    }
    let count = exact_symbol(&output);
    if count != 1 {
        return Err(format!(
            "Actual caller requires one defined dynamic ASan runtime symbol; matches={count}"
        )
        .into());
    }
    println!("defined_dynamic_symbol=__asan_init matches=1");
    Ok(())
}
