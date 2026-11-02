use crate::{Cli, Commands, LanguageCommand, ModelCommand, config};
use nvsf_app::{AppError, Result};

fn invalid(message: impl Into<String>) -> AppError {
    AppError::InvalidConfig(message.into())
}

pub fn validate(cli: &Cli) -> Result<()> {
    match &cli.command {
        Commands::Extract(args) | Commands::Benchmark(args) => {
            config(args.clone(), cli.data_root.as_deref())?;
        }
        Commands::Batch {
            inputs,
            output_dir,
            language,
        } => {
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
        Commands::Export { format, .. } => {
            if !["srt", "vtt", "ass", "json"].contains(&format.as_str()) {
                return Err(invalid("Choose srt, vtt, ass or json"));
            }
        }
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
                "a",
                "--output",
                "b",
                "--language",
                "xx",
            ],
            vec![
                "nvsf",
                "extract",
                "--input",
                "a",
                "--output",
                "b",
                "--threads",
                "0",
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
        ] {
            let cli = Cli::try_parse_from(args).unwrap_or_else(|error| panic!("{error}"));
            assert!(matches!(validate(&cli), Err(AppError::InvalidConfig(_))));
        }
    }
}
