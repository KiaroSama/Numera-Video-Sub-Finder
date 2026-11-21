use crate::{
    Cli, Commands, LanguageCommand, ModelCommand, PreferencesCommand, PreviewCommand,
    ProjectCommand, QueueCommand, RegionsCommand, UpdatesCommand, bulk, config, updates_cmd,
};
use nvsf_app::{AppError, Result};
use nvsf_core::Rect;

fn invalid(message: impl Into<String>) -> AppError {
    AppError::InvalidConfig(message.into())
}

#[allow(clippy::too_many_arguments)]
fn validate_extract_fields(
    language: &str,
    device: &str,
    quality: &str,
    threads: usize,
    start_ms: u64,
    end_ms: Option<u64>,
    min_confidence: f32,
    gpu_required: bool,
    accelerator_required: bool,
    roi: &[Rect],
    exclude: &[Rect],
) -> Result<()> {
    if nvsf_core::languages::find(language).is_none() {
        return Err(invalid("Select a language from the official catalog"));
    }
    if !["cpu", "auto", "cuda", "directml"].contains(&device) {
        return Err(invalid("device must be cpu, auto, cuda or directml"));
    }
    if !["accurate", "balanced", "fast"].contains(&quality) {
        return Err(invalid("quality must be accurate, balanced or fast"));
    }
    if threads == 0 || threads > 16 || roi.len() > 8 || exclude.len() > 32 {
        return Err(invalid(
            "threads must be 1..=16, roi at most 8, exclude at most 32",
        ));
    }
    if end_ms.is_some_and(|end| end <= start_ms)
        || !min_confidence.is_finite()
        || !(0.0..=1.0).contains(&min_confidence)
    {
        return Err(invalid("invalid range/confidence"));
    }
    if device == "cpu" && (gpu_required || accelerator_required) {
        return Err(invalid("CPU contradicts required acceleration"));
    }
    for r in roi.iter().chain(exclude.iter()) {
        r.validate().map_err(|e| invalid(e.to_string()))?;
    }
    Ok(())
}

pub fn validate(cli: &Cli) -> Result<()> {
    match &cli.command {
        Commands::Extract(args) | Commands::Benchmark(args) => {
            config(args.clone(), cli.data_root.as_deref())?;
            if let Some(directory) = args.provider_profile.as_deref() {
                crate::provider_profile::validate_options(directory, &args.device, &args.language)?;
                crate::provider_profile::validate_new_directory(directory)?;
            }
        }
        Commands::Batch {
            inputs,
            output_dir,
            language,
            concurrency,
        } => {
            bulk::validate_concurrency(*concurrency)?;
            if !output_dir.is_dir() {
                return Err(invalid("Batch output directory must exist"));
            }
            if nvsf_core::languages::find(language).is_none() {
                return Err(invalid("Select a language from the official catalog"));
            }
            for input in inputs {
                crate::utf8_path(input)?;
                input
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .ok_or_else(|| invalid("Batch input requires a UTF-8 filename"))?;
            }
        }
        Commands::Bulk(args) => {
            bulk::validate_concurrency(args.concurrency)?;
            if !["srt", "vtt", "ass", "json"].contains(&args.format.as_str()) {
                return Err(invalid("Choose srt, vtt, ass or json"));
            }
            validate_extract_fields(
                &args.language,
                &args.device,
                &args.quality,
                args.threads,
                args.start_ms,
                args.end_ms,
                args.min_confidence,
                args.gpu_required,
                args.accelerator_required,
                &args.roi,
                &args.exclude,
            )?;
            if let Some(dir) = args.output_dir.as_deref()
                && !dir.is_dir()
            {
                return Err(invalid("Bulk --output-dir must be an existing directory"));
            }
            if args.inputs.is_empty() && args.inputs_file.is_none() && args.input_dir.is_none() {
                return Err(invalid(
                    "Bulk mode requires at least one input via --input, --inputs-file or --input-dir",
                ));
            }
        }
        Commands::Export { format, .. } => {
            if !["srt", "vtt", "ass", "json"].contains(&format.as_str()) {
                return Err(invalid("Choose srt, vtt, ass or json"));
            }
        }
        Commands::Project { command } => match command {
            ProjectCommand::Cues { filter, .. } => {
                if !matches!(
                    filter.as_str(),
                    "all" | "unreviewed" | "uncertain" | "edited"
                ) {
                    return Err(invalid(
                        "Filter must be all, unreviewed, uncertain or edited",
                    ));
                }
            }
            ProjectCommand::EditCue {
                text,
                start_ms,
                end_ms,
                ..
            } => {
                if text.is_none() && start_ms.is_none() && end_ms.is_none() {
                    return Err(invalid(
                        "edit-cue requires at least one of --text, --start-ms or --end-ms",
                    ));
                }
            }
            ProjectCommand::ReOcr {
                language, region, ..
            } => {
                if let Some(code) = language.as_deref()
                    && nvsf_core::languages::find(code).is_none()
                {
                    return Err(invalid("Select a language from the official catalog"));
                }
                if let Some(rect) = region {
                    rect.validate().map_err(|e| invalid(e.to_string()))?;
                }
            }
            ProjectCommand::VisualRetry { opt_in, region, .. } => {
                if !opt_in {
                    return Err(invalid("Local visual retry requires explicit --opt-in"));
                }
                if let Some(rect) = region {
                    rect.validate().map_err(|e| invalid(e.to_string()))?;
                }
            }
            ProjectCommand::VisualRetryInstall { opt_in } if !opt_in => {
                return Err(invalid("Visual retry model installation requires --opt-in"));
            }
            _ => {}
        },
        Commands::Regions { command } => match command {
            RegionsCommand::Suggest { language, .. } => {
                if nvsf_core::languages::find(language).is_none() {
                    return Err(invalid("Select a language from the official catalog"));
                }
            }
        },
        Commands::Preview { command } => match command {
            PreviewCommand::Frame { crop, .. } => {
                if let Some(rect) = crop {
                    rect.validate().map_err(|e| invalid(e.to_string()))?;
                }
            }
        },
        Commands::Queue { command } => match command {
            QueueCommand::Add(args) => {
                validate_extract_fields(
                    &args.language,
                    &args.device,
                    &args.quality,
                    args.threads,
                    args.start_ms,
                    args.end_ms,
                    args.min_confidence,
                    args.gpu_required,
                    args.accelerator_required,
                    &args.roi,
                    &args.exclude,
                )?;
            }
            QueueCommand::Clear { state }
                if !matches!(
                    state.as_str(),
                    "all" | "queued" | "interrupted" | "completed" | "cancelled" | "failed"
                ) =>
            {
                return Err(invalid(
                    "State filter must be all, queued, interrupted, completed, cancelled or failed",
                ));
            }
            QueueCommand::Run { concurrency, .. } => {
                bulk::validate_concurrency(*concurrency)?;
            }
            _ => {}
        },
        Commands::Updates { command } => match command {
            UpdatesCommand::Status { format } | UpdatesCommand::Check { format } => {
                updates_cmd::validate_format(format)?;
            }
            UpdatesCommand::Download { consent, .. } => {
                if !consent {
                    return Err(invalid("Update download requires explicit --consent"));
                }
            }
            UpdatesCommand::Verify {
                token,
                file,
                signature,
                version,
                ..
            } => {
                if token.is_none() && (file.is_none() || signature.is_none() || version.is_none()) {
                    return Err(invalid(
                        "updates verify requires --token or all of --file, --signature and --version",
                    ));
                }
            }
            UpdatesCommand::Config { .. } => {}
        },
        Commands::Preferences { command } => match command {
            PreferencesCommand::Get => {}
            PreferencesCommand::Set {
                update_checks,
                reset_last_attempt,
            } => {
                if update_checks.is_none() && !reset_last_attempt {
                    return Err(invalid(
                        "preferences set requires --update-checks or --reset-last-attempt",
                    ));
                }
            }
        },
        Commands::Models { command } => {
            let id = match command {
                ModelCommand::Install { id } => id.as_deref(),
                ModelCommand::Import { id, .. } | ModelCommand::Export { id, .. } => {
                    Some(id.as_str())
                }
                _ => None,
            };
            if let Some(id) = id
                && !nvsf_app::models::specs()?
                    .iter()
                    .any(|model| model.id == id)
            {
                return Err(invalid("Unknown official model identifier"));
            }
        }
        Commands::Languages { command } => {
            let code = match command {
                LanguageCommand::List => None,
                LanguageCommand::Install { code }
                | LanguageCommand::Verify { code }
                | LanguageCommand::Import { code, .. }
                | LanguageCommand::Export { code, .. } => Some(code),
            };
            if code.is_some_and(|code| nvsf_core::languages::find(code).is_none()) {
                return Err(invalid("Select a language from the official catalog"));
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn invalid_configuration_is_classified_before_runtime_or_data_initialization() {
        for args in [
            vec![
                "nvsf",
                "extract",
                "--input",
                "missing.mp4",
                "--output",
                "out.srt",
                "--device",
                "invalid",
            ],
            vec![
                "nvsf",
                "extract",
                "--input",
                "missing.mp4",
                "--output",
                "out.srt",
                "--device",
                "cpu",
                "--gpu-required",
            ],
            vec![
                "nvsf",
                "export",
                "missing.json",
                "out",
                "--format",
                "unknown",
            ],
            vec!["nvsf", "languages", "install", "../invalid-language"],
            vec!["nvsf", "models", "install", "--id", "../unknown"],
            vec!["nvsf", "project", "cues", "p.json", "--filter", "bogus"],
            vec!["nvsf", "project", "edit-cue", "p.json", "--id", "1"],
            vec![
                "nvsf",
                "project",
                "visual-retry",
                "p.json",
                "--id",
                "1",
                "--input",
                "a.mp4",
            ],
            vec!["nvsf", "project", "visual-retry-install"],
            vec![
                "nvsf",
                "regions",
                "suggest",
                "--input",
                "a.mp4",
                "--language",
                "zz",
            ],
            vec!["nvsf", "updates", "download", "--token", "t1"],
            vec!["nvsf", "updates", "status", "--format", "bogus"],
            vec!["nvsf", "updates", "verify"],
            vec!["nvsf", "preferences", "set"],
            vec!["nvsf", "queue", "clear", "--state", "bogus"],
            vec!["nvsf", "queue", "run", "--concurrency", "9"],
            vec!["nvsf", "bulk", "--concurrency", "0", "--input", "a.mp4"],
            vec!["nvsf", "bulk", "--concurrency", "9", "--input", "a.mp4"],
            vec!["nvsf", "bulk"],
            vec![
                "nvsf",
                "batch",
                "a.mp4",
                "--output-dir",
                ".",
                "--concurrency",
                "0",
            ],
            vec![
                "nvsf",
                "batch",
                "a.mp4",
                "--output-dir",
                ".",
                "--concurrency",
                "9",
            ],
        ] {
            let cli = Cli::try_parse_from(args).unwrap_or_else(|error| panic!("{error}"));
            assert!(matches!(validate(&cli), Err(AppError::InvalidConfig(_))));
        }
    }

    #[test]
    fn valid_new_subcommands_parse_and_pass_preflight() {
        for args in [
            vec!["nvsf", "project", "inspect", "p.json"],
            vec![
                "nvsf",
                "project",
                "cues",
                "p.json",
                "--query",
                "hi",
                "--filter",
                "unreviewed",
                "--offset",
                "0",
                "--limit",
                "10",
            ],
            vec![
                "nvsf", "project", "edit-cue", "p.json", "--id", "1", "--text", "fixed",
            ],
            vec!["nvsf", "project", "accept-cue", "p.json", "--id", "1"],
            vec!["nvsf", "project", "revert-cue", "p.json", "--id", "1"],
            vec![
                "nvsf",
                "project",
                "split-cue",
                "p.json",
                "--id",
                "1",
                "--at-ms",
                "1500",
            ],
            vec![
                "nvsf",
                "project",
                "merge-cues",
                "p.json",
                "--first",
                "1",
                "--second",
                "2",
            ],
            vec![
                "nvsf",
                "project",
                "visual-retry",
                "p.json",
                "--id",
                "1",
                "--input",
                "a.mp4",
                "--opt-in",
            ],
            vec!["nvsf", "project", "visual-retry-install", "--opt-in"],
            vec!["nvsf", "project", "save", "p.json"],
            vec!["nvsf", "project", "history", "--limit", "50"],
            vec![
                "nvsf",
                "regions",
                "suggest",
                "--input",
                "a.mp4",
                "--language",
                "en",
            ],
            vec![
                "nvsf",
                "preview",
                "frame",
                "--input",
                "a.mp4",
                "--at-ms",
                "1000",
                "--output",
                "out.png",
                "--crop",
                "0.1,0.7,0.8,0.2",
            ],
            vec!["nvsf", "queue", "list"],
            vec![
                "nvsf", "queue", "add", "--input", "a.mp4", "--output", "a.srt",
            ],
            vec!["nvsf", "queue", "remove", "--id", "cli-1"],
            vec!["nvsf", "queue", "clear", "--state", "completed"],
            vec!["nvsf", "queue", "run", "--concurrency", "2"],
            vec!["nvsf", "updates", "status"],
            vec!["nvsf", "updates", "check"],
            vec!["nvsf", "updates", "config", "--enabled", "false"],
            vec!["nvsf", "updates", "download", "--token", "t1", "--consent"],
            vec!["nvsf", "updates", "verify", "--token", "t1"],
            vec!["nvsf", "preferences", "get"],
            vec!["nvsf", "preferences", "set", "--update-checks", "true"],
            vec![
                "nvsf",
                "bulk",
                "--input",
                "a.mp4",
                "--input",
                "b.mp4",
                "--concurrency",
                "4",
                "--format",
                "vtt",
            ],
            vec![
                "nvsf",
                "batch",
                "a.mp4",
                "--output-dir",
                ".",
                "--concurrency",
                "2",
            ],
        ] {
            let cli = Cli::try_parse_from(args).unwrap_or_else(|error| panic!("{error}"));
            assert!(validate(&cli).is_ok());
        }
    }
}
