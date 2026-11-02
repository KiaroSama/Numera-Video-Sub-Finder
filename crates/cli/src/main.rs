mod preflight;
mod status;

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
#[derive(Clone, clap::Args)]
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
    path.to_str().map(str::to_owned).ok_or_else(||AppError::InvalidConfig("This operation requires a valid UTF-8 path; rename the input or select another destination".into()))
}
fn config(args: ExtractArgs, root: Option<&Path>) -> Result<ExtractConfig> {
    let config = ExtractConfig {
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
    };
    config
        .validate()
        .map_err(|error| AppError::InvalidConfig(error.to_string()))?;
    Ok(config)
}
fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = if error.print().is_ok() {
                status::parse(&error)
            } else {
                4
            };
            std::process::exit(code);
        }
    };
    let code = match run(cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("ERROR: {error}");
            status::failure(&error)
        }
    };
    std::process::exit(code);
}
fn run(cli: Cli) -> Result<i32> {
    if let Commands::NativeRequest { request } = &cli.command {
        return nvsf_app::native_request::run(request).map(|()| 0);
    }
    if let Commands::Worker { request, control } = &cli.command {
        return run_worker(request, control).map(|()| 0);
    }
    preflight::validate(&cli)?;
    let paths = Paths::new(cli.data_root.as_deref())?;
    let log = Log::new(&paths, "nvsf");
    let mut exit_code = 0;
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
            exit_code = status::project(&project);
            println!(
                "{}",
                serde_json::json!({"cues":project.cues.len(),"completed":project.completed,"review_needed":exit_code == 2,"elapsed_ms":started.elapsed().as_millis(),"warnings":project.warnings})
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
            let project = nvsf_app::extract(p.config, &control, |p| {
                eprintln!("{} {}ms", p.stage, p.position_ms)
            })?;
            exit_code = status::project(&project);
            println!(
                "{}",
                serde_json::json!({"completed":project.completed,"cues":project.cues.len(),"review_needed":exit_code == 2})
            );
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
                return Err(AppError::InvalidConfig(
                    "Batch output directory must exist".into(),
                ));
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
                let project = nvsf_app::extract(c, &control, |p| {
                    eprintln!("Batch{} {}ms", i + 1, p.position_ms)
                })?;
                exit_code = exit_code.max(status::project(&project));
                println!(
                    "{}",
                    serde_json::json!({"item":i + 1,"completed":project.completed,"cues":project.cues.len(),"review_needed":status::project(&project) == 2})
                );
            }
        }
        Commands::Worker { .. } | Commands::NativeRequest { .. } => {
            return Err(nvsf_app::fail("Invalid worker invocation"));
        }
    }
    log.event(
        "INFO",
        "CLI",
        &format!("Command completed exit_code={exit_code}"),
    );
    Ok(exit_code)
}

fn run_worker(request: &Path, control_dir: &Path) -> Result<()> {
    nvsf_app::worker::guard_parent_lifetime()?;
    use worker::protocol;
    let job_id = protocol::job_id(control_dir)?;
    let config: ExtractConfig = protocol::read(request, job_id, protocol::CONTROL_LIMIT)?;
    config.validate()?;
    let control = Control::default();
    control.pause.store(
        protocol::flag(control_dir, job_id, "pause")?,
        Ordering::Relaxed,
    );
    control.cancel.store(
        protocol::flag(control_dir, job_id, "cancel")?,
        Ordering::Relaxed,
    );
    let flags = control.clone();
    let dir = control_dir.to_path_buf();
    let watcher_job = job_id.to_owned();
    let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done = stopped.clone();
    let watcher = std::thread::spawn(move || -> Result<()> {
        while !done.load(Ordering::Relaxed) {
            let update = (|| -> Result<()> {
                if protocol::flag(&dir, &watcher_job, "cancel")? {
                    flags.cancel.store(true, Ordering::Relaxed);
                }
                flags.pause.store(
                    protocol::flag(&dir, &watcher_job, "pause")?,
                    Ordering::Relaxed,
                );
                Ok(())
            })();
            if let Err(error) = update {
                flags.cancel.store(true, Ordering::Relaxed);
                return Err(error);
            }
            std::thread::park_timeout(std::time::Duration::from_millis(50));
        }
        Ok(())
    });
    let mut emission_error = None;
    let output = nvsf_app::extraction::run(config, &control, |p| {
        if emission_error.is_none()
            && let Err(error) = emit(job_id, &worker::Event::Progress(p))
        {
            emission_error = Some(error);
            control.cancel.store(true, Ordering::Relaxed);
        }
    });
    stopped.store(true, Ordering::Relaxed);
    watcher.thread().unpark();
    watcher
        .join()
        .map_err(|_| nvsf_app::fail("Worker control watcher failed"))??;
    if let Some(error) = emission_error {
        return Err(error);
    }
    match output {
        Ok(p) => {
            emit(job_id, &worker::Event::Completed(Box::new(p)))?;
            Ok(())
        }
        Err(e) => {
            emit(
                job_id,
                &worker::Event::Failure(protocol::Failure::from_error(&e)),
            )?;
            Err(e)
        }
    }
}
fn emit(job_id: &str, event: &worker::Event) -> Result<()> {
    let bytes = worker::protocol::encode(job_id, event, worker::protocol::EVENT_LIMIT - 1)?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&bytes)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}
