mod bulk;
mod cli;
mod frame_overlay;
mod media_cmd;
mod preflight;
mod project;
mod project_ops;
mod provider_profile;
mod queue_cmd;
mod resume;
mod status;
mod updates_cmd;
mod worker_cmd;

pub(crate) use cli::*;

use clap::Parser;
use nvsf_app::{AppError, Control, Paths, Result, logging::Log, media, models, storage, worker};
use nvsf_core::{ExtractConfig, Rect};
use std::{path::Path, sync::atomic::Ordering};

pub(crate) fn utf8_path(path: &Path) -> Result<String> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        AppError::InvalidConfig(
            "This operation requires a valid UTF-8 path; rename the input or select another destination".into(),
        )
    })
}

pub(crate) fn config(args: ExtractArgs, root: Option<&Path>) -> Result<ExtractConfig> {
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

const EMBEDDED_LOGO_SVG: &[u8] = include_bytes!("../../../assets/logo.svg");

fn main() {
    std::hint::black_box(EMBEDDED_LOGO_SVG);
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
    if let Commands::Worker {
        command,
        request,
        control,
        provider_profile,
        resume_project,
        srt_bom,
        srt_crlf,
    } = &cli.command
    {
        return match (command, request, control) {
            (Some(crate::cli::WorkerCommand::PreviewDaemon { input }), None, None) => {
                worker_cmd::run_preview_daemon(input, cli.data_root.as_deref())
            }
            (None, Some(request), Some(control)) => worker_cmd::run_worker(
                request,
                control,
                provider_profile.as_deref(),
                resume_project.as_deref(),
                nvsf_app::extraction::SrtOptions {
                    bom: *srt_bom,
                    crlf: *srt_crlf,
                },
            ),
            _ => Err(nvsf_app::fail("Invalid worker invocation")),
        }
        .map(|()| 0);
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
            let profile_directory = args.provider_profile.clone();
            let config = config(args, cli.data_root.as_deref())?;
            let progress = |p: nvsf_core::Progress| {
                eprintln!(
                    "{} {}ms cues={} provider={}",
                    p.stage, p.position_ms, p.cues, p.provider
                );
            };
            let project = if let Some(directory) = profile_directory.as_deref() {
                worker::supervise_profiled(config, &control, progress, directory)?
            } else {
                nvsf_app::extract(config, &control, progress)?
            };
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
            let project = resume::run(&project, &control)?;
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
            concurrency,
        } => {
            if !output_dir.is_dir() {
                return Err(AppError::InvalidConfig(
                    "Batch output directory must exist".into(),
                ));
            }
            if concurrency > 1 {
                exit_code = bulk::run_batch_concurrent(
                    &inputs,
                    &output_dir,
                    &language,
                    concurrency,
                    cli.data_root.as_deref(),
                    &control,
                )?;
            } else {
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
        }
        Commands::Bulk(args) => {
            exit_code = bulk::run_bulk(*args, cli.data_root.as_deref(), &control)?;
        }
        Commands::Project { command } => {
            exit_code = project::run(&paths, command, &control)?;
        }
        Commands::Regions { command } => {
            media_cmd::run_regions(&paths, command, &control)?;
        }
        Commands::Preview { command } => {
            media_cmd::run_preview(&paths, command, &control)?;
        }
        Commands::Queue { command } => {
            exit_code = queue_cmd::run(&paths, command, &control)?;
        }
        Commands::Updates { command } => {
            updates_cmd::run_updates(&paths, command, &control)?;
        }
        Commands::Preferences { command } => {
            updates_cmd::run_preferences(&paths, command)?;
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
