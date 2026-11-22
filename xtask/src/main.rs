mod ci_authenticode;
mod ci_bundle;
mod ci_coverage;
mod ci_crt;
mod ci_frontend;
mod ci_fuzz;
mod ci_guard;
mod ci_media_sanitizer;
mod ci_native;
mod ci_native_helpers;
mod ci_notices;
mod ci_tasks;
mod coverage_scope;
mod executable_icons;
mod extraction_witness;
mod fixture;
mod gate;
mod gui;
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
mod source_policy;
mod specialist_witness;
mod subtitle_interop;
mod update_assets;
mod visual_witness;
mod worker_faults;
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
    Specialist {
        #[arg(long)]
        fixture_dir: PathBuf,
        #[arg(long)]
        data_root: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    VisualModel {
        #[arg(long)]
        fixture_dir: PathBuf,
        #[arg(long)]
        data_root: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    WorkerFaults {
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
    ExecutableIcons {
        #[arg(long)]
        binary: PathBuf,
        #[arg(long)]
        icon: PathBuf,
        #[arg(long)]
        output: PathBuf,
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
    VisualModels {
        #[arg(long)]
        data_root: PathBuf,
    },
    SourcePolicy,
    CiTests {
        #[arg(long)]
        target_id: String,
    },
    VirtualDesktop {
        #[arg(long)]
        executable: PathBuf,
        #[arg(long)]
        arguments_json: String,
    },
    FrontendTools,
    CiCrt {
        #[arg(long)]
        mode: String,
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long)]
        arch: Option<String>,
    },
    CiNotices {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        destination: PathBuf,
    },
    CiCoverage {
        #[arg(long)]
        phase: String,
        #[arg(long)]
        target: Option<String>,
    },
    CiFrontend {
        #[arg(long)]
        e2e: bool,
    },
    CiGuard {
        #[arg(long)]
        name: String,
        #[arg(long)]
        wall_seconds: u32,
        #[arg(long)]
        idle_seconds: u32,
        #[arg(long)]
        working_directory: Option<PathBuf>,
        #[arg(long)]
        capture_stdout_path: Option<PathBuf>,
        #[arg(long)]
        progress_directory: Option<PathBuf>,
        #[arg(long, value_parser = ["none", "stall"], default_value = "none")]
        progress_contract: String,
        #[arg(long, visible_alias = "file-path")]
        executable: PathBuf,
        #[arg(last = true)]
        arguments: Vec<std::ffi::OsString>,
    },
    CiGuardTests,
    CiNative {
        #[arg(long)]
        target_id: String,
        #[arg(long)]
        stage_media_adapter: bool,
    },
    CiBundle {
        #[arg(long)]
        target_id: String,
        #[arg(long, default_value = ".ci-work/packages")]
        packages_root: PathBuf,
    },
    CiAuthenticode {
        #[arg(long, value_parser = ["enable", "disable", "sign", "verify", "negative-checks"])]
        action: String,
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        certificate_sha256: Option<String>,
    },
    CiAuthenticodeTests,
    CiWindowsSigning {
        #[arg(long)]
        path: PathBuf,
    },
    CiFuzz,
    CiMediaSanitizer,
    CiNoticeTests,
    CiFrontendTests,
}
fn main() {
    if let Err(e) = run() {
        eprintln!("[ERROR] [CI] {e}");
        std::process::exit(
            e.downcast_ref::<ci_guard::Failure>()
                .map_or(1, |failure| failure.code),
        );
    }
}
fn run() -> Result<(), Box<dyn Error>> {
    if let Some(code) = ci_guard::dispatch()? {
        std::process::exit(code);
    }
    if ci_media_sanitizer::dispatch()? || worker_faults::dispatch_fixture()? {
        return Ok(());
    }
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
        Cmd::Specialist {
            fixture_dir,
            data_root,
            output,
        } => specialist_witness::verify(&fixture_dir, &data_root, &output),
        Cmd::VisualModel {
            fixture_dir,
            data_root,
            output,
        } => visual_witness::verify(&fixture_dir, &data_root, &output),
        Cmd::WorkerFaults { output } => worker_faults::verify(&output),
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
        Cmd::ExecutableIcons {
            binary,
            icon,
            output,
        } => executable_icons::verify(&binary, &icon, &output),
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
        Cmd::VisualModels { data_root } => visual_witness::provision(&data_root),
        Cmd::SourcePolicy => source_policy::verify(),
        Cmd::CiTests { target_id } => ci_tasks::ci_tests(&target_id),
        Cmd::VirtualDesktop {
            executable,
            arguments_json,
        } => ci_tasks::virtual_desktop(&executable, &arguments_json),
        Cmd::FrontendTools => ci_tasks::frontend_tools(),
        Cmd::CiCrt { mode, input, arch } => ci_crt::run(&mode, input.as_deref(), arch.as_deref()),
        Cmd::CiNotices {
            source,
            destination,
        } => ci_notices::run(&source, &destination),
        Cmd::CiCoverage { phase, target } => ci_coverage::run(&phase, target.as_deref()),
        Cmd::CiFrontend { e2e } => ci_frontend::run(e2e),
        Cmd::CiGuard {
            name,
            wall_seconds,
            idle_seconds,
            working_directory,
            capture_stdout_path,
            progress_directory,
            progress_contract,
            executable,
            arguments,
        } => {
            let mut command = std::process::Command::new(executable);
            command.args(arguments);
            if let Some(directory) = working_directory {
                command.current_dir(directory);
            }
            ci_guard::run(
                ci_guard::Guard {
                    name: &name,
                    wall_s: wall_seconds,
                    idle_s: idle_seconds,
                    capture: capture_stdout_path.as_deref(),
                    progress: progress_directory.as_deref(),
                    stall: progress_contract == "stall",
                },
                command,
            )?
            .require_success(&name)
        }
        Cmd::CiGuardTests => ci_guard::contracts(),
        Cmd::CiNative {
            target_id,
            stage_media_adapter,
        } => ci_native::execute(&target_id, stage_media_adapter),
        Cmd::CiBundle {
            target_id,
            packages_root,
        } => ci_bundle::execute(&target_id, &packages_root),
        Cmd::CiAuthenticode {
            action,
            path,
            certificate_sha256,
        } => ci_authenticode::identity(&action, path.as_deref(), certificate_sha256.as_deref()),
        Cmd::CiAuthenticodeTests => ci_authenticode::contracts(),
        Cmd::CiWindowsSigning { path } => ci_authenticode::sign_allowlisted(&path),
        Cmd::CiFuzz => ci_fuzz::run(),
        Cmd::CiMediaSanitizer => ci_media_sanitizer::run(),
        Cmd::CiNoticeTests => ci_notices::contracts(),
        Cmd::CiFrontendTests => ci_frontend::contracts(),
    };
    writeln!(
        log,
        "[{}] [{}] [CI] xtask finished",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
        if result.is_ok() { "INFO" } else { "ERROR" }
    )?;
    result
}
