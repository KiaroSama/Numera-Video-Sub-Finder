mod coverage_scope;
mod fixture;
mod gate;
mod gui;
#[cfg(any(target_os = "macos", test))]
mod gui_debugger;
mod gui_failure;
mod gui_visual;
mod maintenance;
mod multilingual;
mod ocr_evidence;
mod ocr_replay;
mod package;
mod process;
mod production;
#[cfg(any(target_os = "linux", test))]
mod production_supervisor;
mod quality;
mod runtime_pack;
mod update_assets;
use clap::{Parser, Subcommand};
use std::{error::Error, path::PathBuf};

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Cmd,
}
#[derive(Subcommand)]
enum Cmd {
    Fixtures {
        #[arg(long)]
        ffmpeg: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    OcrEvidence {
        #[arg(long)]
        ffmpeg: PathBuf,
        #[arg(long)]
        fixture_dir: PathBuf,
        #[arg(long)]
        data_root: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Smoke {
        #[arg(long)]
        cli: PathBuf,
        #[arg(long)]
        fixture_dir: PathBuf,
        #[arg(long)]
        data_root: PathBuf,
    },
    Gui {
        #[arg(long)]
        gui: PathBuf,
        #[arg(long)]
        fixture_dir: PathBuf,
        #[arg(long)]
        data_root: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    GuiFault {
        #[arg(long)]
        output: PathBuf,
    },
    #[cfg(target_os = "linux")]
    ProductionParent {
        #[arg(long)]
        gui: PathBuf,
        #[arg(long)]
        state: PathBuf,
    },
    Production {
        #[arg(long)]
        gui: PathBuf,
        #[arg(long)]
        data_root: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Maintenance {
        #[arg(long)]
        cli: PathBuf,
        #[arg(long)]
        runtimes: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Multilingual {
        #[arg(long)]
        cli: PathBuf,
        #[arg(long)]
        ffmpeg: PathBuf,
        #[arg(long)]
        data_root: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    RuntimePack {
        #[arg(long)]
        target: String,
        #[arg(long)]
        runtimes: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    UpdateAssets {
        #[arg(long)]
        target: String,
        #[arg(long)]
        packages: PathBuf,
    },
    Package {
        #[arg(long)]
        target: String,
        #[arg(long)]
        cli: PathBuf,
        #[arg(long)]
        gui: PathBuf,
        #[arg(long)]
        runtimes: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Standalone {
        #[arg(long)]
        target: String,
        #[arg(long)]
        packages: PathBuf,
        #[arg(long)]
        fixture_dir: PathBuf,
    },
    Gate {
        #[arg(long)]
        reports: PathBuf,
        #[arg(long)]
        source_sha: String,
    },
    Coverage {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Mutation {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    ReleaseGate {
        #[arg(long)]
        reports: PathBuf,
        #[arg(long)]
        source_sha: String,
    },
    RuntimeModels {
        #[arg(long)]
        data_root: PathBuf,
    },
}
fn main() {
    if let Err(e) = run() {
        eprintln!("[ERROR] [CI] {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn Error>> {
    let log_dir = PathBuf::from(".ci-work/logs");
    std::fs::create_dir_all(&log_dir)?;
    let stamp = chrono::Utc::now().format("%Y-%m-%d_%H-%M-%S_UTC");
    let path = log_dir.join(format!("xtask_{stamp}_{}.log", std::process::id()));
    let mut log = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?;
    use std::io::Write;
    writeln!(
        log,
        "[{}] [INFO] [CI] xtask started",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    )?;
    let result = match Args::parse().command {
        Cmd::Fixtures { ffmpeg, output } => fixture::generate(&ffmpeg, &output),
        Cmd::OcrEvidence {
            ffmpeg,
            fixture_dir,
            data_root,
            output,
        } => ocr_evidence::capture(&ffmpeg, &fixture_dir, &data_root, &output),
        Cmd::Smoke {
            cli,
            fixture_dir,
            data_root,
        } => fixture::smoke(&cli, &fixture_dir, &data_root),
        Cmd::Gui {
            gui,
            fixture_dir,
            data_root,
            output,
        } => gui::smoke(&gui, &fixture_dir, &data_root, &output),
        Cmd::GuiFault { output } => gui_failure::finalize(&output),
        #[cfg(target_os = "linux")]
        Cmd::ProductionParent { gui, state } => production_supervisor::run(&gui, &state),
        Cmd::Production {
            gui,
            data_root,
            output,
        } => production::smoke(&gui, &data_root, &output),
        Cmd::Maintenance {
            cli,
            runtimes,
            output,
        } => maintenance::verify(&cli, &runtimes, &output),
        Cmd::Multilingual {
            cli,
            ffmpeg,
            data_root,
            output,
        } => multilingual::verify(&cli, &ffmpeg, &data_root, &output),
        Cmd::RuntimePack {
            target,
            runtimes,
            output,
        } => runtime_pack::generate(&target, &runtimes, &output),
        Cmd::UpdateAssets { target, packages } => update_assets::generate(&target, &packages),
        Cmd::Package {
            target,
            cli,
            gui,
            runtimes,
            output,
        } => package::assemble(&target, &cli, &gui, &runtimes, &output),
        Cmd::Standalone {
            target,
            packages,
            fixture_dir,
        } => package::standalone(&target, &packages, &fixture_dir),
        Cmd::Gate {
            reports,
            source_sha,
        } => gate::validate(&reports, &source_sha),
        Cmd::Coverage { input, output } => quality::coverage(&input, &output),
        Cmd::Mutation { input, output } => quality::mutation(&input, &output),
        Cmd::ReleaseGate {
            reports,
            source_sha,
        } => gate::release(&reports, &source_sha),
        Cmd::RuntimeModels { data_root } => fixture::provision_models(&data_root),
    };
    writeln!(
        log,
        "[{}] [{}] [CI] xtask finished",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
        if result.is_ok() { "INFO" } else { "ERROR" }
    )?;
    result
}
