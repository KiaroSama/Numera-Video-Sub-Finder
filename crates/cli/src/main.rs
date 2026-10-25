use clap::{Parser, Subcommand};
use nvsf_app::{AppError, Control, Paths, Result, logging::Log, media, models, storage, worker};
use nvsf_core::{ExtractConfig, Rect};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

#[derive(Parser)]
#[command(version, about = "Extract visible video subtitles locally")]
struct Cli {
    #[arg(long, global = true)]
    data_root: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    Extract(ExtractArgs),
    Probe {
        input: PathBuf,
    },
    Doctor {
        #[arg(long, default_value = "json", value_parser=["json"])]
        format: String,
    },
    /// Verify native APIs or repair this application without changing system software.
    Dependencies {
        #[arg(long)]
        repair: bool,
        #[arg(long, requires = "repair")]
        consent: bool,
    },
    Models {
        #[command(subcommand)]
        command: ModelCommand,
    },
    Languages {
        #[command(subcommand)]
        command: LanguageCommand,
    },
    Resume {
        project: PathBuf,
    },
    Export {
        project: PathBuf,
        output: PathBuf,
        #[arg(long, default_value = "srt")]
        format: String,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        bom: bool,
        #[arg(long)]
        crlf: bool,
    },
    Batch {
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        #[arg(long)]
        output_dir: PathBuf,
        #[arg(long, default_value = "en")]
        language: String,
    },
    Benchmark(ExtractArgs),
    #[command(hide = true)]
    NativeRequest {
        #[arg(long)]
        request: PathBuf,
    },
    #[command(hide = true)]
    Worker {
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        control: PathBuf,
    },
}
#[derive(Subcommand)]
enum ModelCommand {
    List,
    Install {
        #[arg(long)]
        id: Option<String>,
    },
    Verify,
    Import {
        id: String,
        path: PathBuf,
    },
    Export {
        id: String,
        path: PathBuf,
    },
}
#[derive(Subcommand)]
enum LanguageCommand {
    List,
    Install { code: String },
    Verify { code: String },
    Import { code: String, path: PathBuf },
    Export { code: String, path: PathBuf },
}
#[derive(clap::Args)]
struct ExtractArgs {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    output: PathBuf,
    #[arg(long, alias = "languages", default_value = "en")]
    language: String,
    #[arg(long, default_value = "auto")]
    device: String,
    #[arg(long, default_value = "balanced")]
    quality: String,
    #[arg(long)]
    roi: Vec<Rect>,
    #[arg(long)]
    exclude: Vec<Rect>,
    #[arg(long, default_value_t = 0)]
    start_ms: u64,
    #[arg(long)]
    end_ms: Option<u64>,
    #[arg(long, default_value_t = 2)]
    threads: usize,
    #[arg(long)]
    overwrite: bool,
    #[arg(long)]
    normalize_persian: bool,
    #[arg(long)]
    gpu_required: bool,
    #[arg(long)]
    accelerator_required: bool,
}
fn utf8_path(path: &Path) -> Result<String> {
    path.to_str().map(str::to_owned).ok_or_else(||nvsf_app::fail("This operation requires a valid UTF-8 path; rename the input or select another destination"))
}
fn config(args: ExtractArgs, root: Option<&Path>) -> Result<ExtractConfig> {
    Ok(ExtractConfig {
        input: utf8_path(&args.input)?,
        output: utf8_path(&args.output)?,
        data_root: root.map(utf8_path).transpose()?,
        language: args.language,
        device: args.device,
        quality: args.quality,
        regions: if args.roi.is_empty() {
            vec![Rect::default()]
        } else {
            args.roi
        },
        exclusions: args.exclude,
        start_ms: args.start_ms,
        end_ms: args.end_ms,
        threads: args.threads,
        overwrite: args.overwrite,
        normalize_persian: args.normalize_persian,
        gpu_required: args.gpu_required,
        accelerator_required: args.accelerator_required,
        ..Default::default()
    })
}
fn main() {
    let cli = Cli::parse();
    let result = run(cli);
    if let Err(e) = result {
        eprintln!("ERROR: {e}");
        std::process::exit(match e {
            AppError::Core(_) => 3,
            AppError::Cancelled => 5,
            AppError::Provider(_) => 6,
            _ => 4,
        });
    }
}
fn run(cli: Cli) -> Result<()> {
    if let Commands::NativeRequest { request } = &cli.command {
        return nvsf_app::native_request::run(request);
    }
    if let Commands::Worker { request, control } = &cli.command {
        return run_worker(request, control);
    }
    let paths = Paths::new(cli.data_root.as_deref())?;
    let log = Log::new(&paths, "nvsf");
    let control = Control::default();
    let cancelled = control.cancel.clone();
    ctrlc::set_handler(move || cancelled.store(true, Ordering::Relaxed))
        .map_err(|e| nvsf_app::fail(e.to_string()))?;
    match cli.command {
        Commands::Extract(args) | Commands::Benchmark(args) => {
            let started = std::time::Instant::now();
            let project =
                nvsf_app::extract(config(args, cli.data_root.as_deref())?, &control, |p| {
                    eprintln!(
                        "{} {}ms cues={} provider={}",
                        p.stage, p.position_ms, p.cues, p.provider
                    );
                })?;
            println!(
                "{}",
                serde_json::json!({"cues":project.cues.len(),"completed":project.completed,"elapsed_ms":started.elapsed().as_millis(),"warnings":project.warnings})
            );
        }
        Commands::Probe { input } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&media::MediaRuntime::load(&paths)?.probe(&input)?)?
            )
        }
        Commands::Doctor { format: _ } => println!(
            "{}",
            serde_json::json!({"version":env!("CARGO_PKG_VERSION"),"source_sha":nvsf_core::BUILD_SOURCE_SHA,"os":std::env::consts::OS,"architecture":std::env::consts::ARCH,"data_root":paths.root,"runtimes":paths.runtimes,"dependencies":nvsf_app::runtimes::diagnose(&paths),"models":models::catalog(&paths)?,"acceleration_qualification":"not established by enumeration"})
        ),
        Commands::Dependencies { repair, consent } => {
            let diagnostics = nvsf_app::runtimes::diagnose(&paths);
            if !repair {
                println!("{}", serde_json::to_string_pretty(&diagnostics)?);
            } else if !consent {
                return Err(nvsf_app::fail(
                    "Dependency repair requires --repair --consent; no payload downloaded",
                ));
            } else if diagnostics.iter().all(|row| row.state == "verified") {
                println!("Existing native APIs and versions verified; no payload downloaded");
            } else {
                log.event(
                    "INFO",
                    "RUNTIMES",
                    "Explicit app-local dependency repair requested",
                );
                let (manifest, signature, base) = nvsf_app::runtimes::fetch()?;
                nvsf_app::runtimes::repair(
                    &paths,
                    &manifest,
                    &signature,
                    &base,
                    consent,
                    &control.cancel,
                )?;
                println!(
                    "Signed native dependencies installed; restart before extraction. System software unchanged."
                );
            }
        }
        Commands::Models { command } => match command {
            ModelCommand::List => println!(
                "{}",
                serde_json::to_string_pretty(&models::catalog(&paths)?)?
            ),
            ModelCommand::Install { id } => {
                models::install(&paths, id.as_deref(), &control.cancel)?;
                println!("Models downloaded and hash verified");
            }
            ModelCommand::Verify => {
                for m in models::specs()? {
                    if m.id != "ja-specialist" || paths.models.join("ja-specialist.onnx").exists() {
                        models::verify(&paths, &m.id)?;
                    }
                }
                for language in ["eng", "fas", "jpn"] {
                    models::verify_tessdata(&paths, language)?;
                }
                println!("Baseline and installed optional model hashes verified");
            }
            ModelCommand::Import { id, path } => models::import_model(&paths, &id, &path)?,
            ModelCommand::Export { id, path } => models::export_model(&paths, &id, &path)?,
        },
        Commands::Languages { command } => match command {
            LanguageCommand::List => println!(
                "{}",
                serde_json::to_string_pretty(&nvsf_app::languages::catalog(&paths))?
            ),
            LanguageCommand::Install { code } => {
                log.event(
                    "INFO",
                    "LANGUAGES",
                    "Explicit selected language provisioning requested",
                );
                nvsf_app::languages::install(&paths, &code, &control.cancel)?;
                println!("Selected language pack downloaded and hash verified");
            }
            LanguageCommand::Verify { code } => {
                nvsf_app::languages::verify(&paths, &code)?;
                println!("Language pack hash verified; subtitle quality is separate");
            }
            LanguageCommand::Import { code, path } => {
                nvsf_app::languages::import(&paths, &code, &path, &control.cancel)?
            }
            LanguageCommand::Export { code, path } => {
                nvsf_app::languages::export(&paths, &code, &path)?
            }
        },
        Commands::Resume { project } => {
            let p = storage::load_project(&project)?;
            nvsf_app::extract(p.config, &control, |p| {
                eprintln!("{} {}ms", p.stage, p.position_ms)
            })?;
        }
        Commands::Export {
            project,
            output,
            format,
            overwrite,
            bom,
            crlf,
        } => nvsf_app::export_project(
            &storage::load_project(&project)?,
            &output,
            &format,
            overwrite,
            bom,
            crlf,
        )?,
        Commands::Batch {
            inputs,
            output_dir,
            language,
        } => {
            if !output_dir.is_dir() {
                return Err(nvsf_app::fail("Batch output directory must exist"));
            }
            for (i, input) in inputs.iter().enumerate() {
                let stem = input
                    .file_stem()
                    .ok_or_else(|| nvsf_app::fail("Input has no filename"))?
                    .to_str()
                    .ok_or_else(|| nvsf_app::fail("Batch filename must be UTF-8"))?;
                let output = output_dir.join(format!("{stem}-{}.srt", i + 1));
                let c = ExtractConfig {
                    input: utf8_path(input)?,
                    output: utf8_path(&output)?,
                    language: language.clone(),
                    data_root: cli.data_root.as_ref().map(|p| utf8_path(p)).transpose()?,
                    ..Default::default()
                };
                nvsf_app::extract(c, &control, |p| {
                    eprintln!("Batch{} {}ms", i + 1, p.position_ms)
                })?;
            }
        }
        Commands::Worker { .. } | Commands::NativeRequest { .. } => {
            return Err(nvsf_app::fail("Invalid worker invocation"));
        }
    }
    log.event("INFO", "CLI", "Command completed");
    Ok(())
}

fn run_worker(request: &Path, control_dir: &Path) -> Result<()> {
    nvsf_app::worker::guard_parent_lifetime()?;
    if request.metadata()?.len() > 1_048_576 {
        return Err(nvsf_app::fail("Worker request exceeds1MiB"));
    }
    let config: ExtractConfig = serde_json::from_slice(&std::fs::read(request)?)?;
    let control = Control::default();
    control
        .pause
        .store(control_dir.join("pause").exists(), Ordering::Relaxed);
    control
        .cancel
        .store(control_dir.join("cancel").exists(), Ordering::Relaxed);
    let flags = control.clone();
    let dir = control_dir.to_path_buf();
    let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done = stopped.clone();
    let watcher = std::thread::spawn(move || {
        while !done.load(Ordering::Relaxed) {
            flags
                .cancel
                .store(dir.join("cancel").exists(), Ordering::Relaxed);
            flags
                .pause
                .store(dir.join("pause").exists(), Ordering::Relaxed);
            std::thread::park_timeout(std::time::Duration::from_millis(50));
        }
    });
    let output = nvsf_app::extraction::run(config, &control, |p| emit(&worker::Event::Progress(p)));
    stopped.store(true, Ordering::Relaxed);
    watcher.thread().unpark();
    let _ = watcher.join();
    match output {
        Ok(p) => {
            emit(&worker::Event::Completed(Box::new(p)));
            Ok(())
        }
        Err(e) => {
            emit(&worker::Event::Failed(e.to_string()));
            Err(e)
        }
    }
}
fn emit(event: &worker::Event) {
    match serde_json::to_string(event) {
        Ok(text) => {
            let mut stdout = std::io::stdout().lock();
            let _ = writeln!(stdout, "{text}");
            let _ = stdout.flush();
        }
        Err(e) => eprintln!("ERROR: Worker event serialization failed: {e}"),
    }
}
