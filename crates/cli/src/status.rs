use nvsf_app::AppError;
use nvsf_core::Project;

pub fn parse(error: &clap::Error) -> i32 {
    if error.use_stderr() { 3 } else { 0 }
}

pub fn failure(error: &AppError) -> i32 {
    match error {
        AppError::InvalidConfig(_) => 3,
        AppError::Cancelled => 5,
        AppError::Provider(_) => 6,
        _ => 4,
    }
}

pub fn project(project: &Project) -> i32 {
    if project.cues.iter().any(|cue| !cue.reviewed) {
        2
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Cli;
    use clap::Parser;

    #[test]
    fn help_and_version_are_success_but_invalid_arguments_are_configuration_errors() {
        for args in [vec!["nvsf", "--help"], vec!["nvsf", "--version"]] {
            let error = Cli::try_parse_from(args).err().unwrap();
            assert_eq!(parse(&error), 0);
            assert!(!error.use_stderr());
        }
        for args in [
            vec!["nvsf"],
            vec!["nvsf", "unknown-command"],
            vec!["nvsf", "extract", "--threads", "not-a-number"],
        ] {
            let error = Cli::try_parse_from(args).err().unwrap();
            assert_eq!(parse(&error), 3);
            assert!(error.use_stderr());
        }
    }

    #[test]
    fn completed_projects_use_the_existing_unreviewed_cue_policy() {
        use nvsf_core::{Cue, ExtractConfig, MediaInfo};
        let mut project = Project {
            schema: 1,
            source_hash: "a".repeat(64),
            config_hash: "b".repeat(64),
            model_hash: "c".repeat(64),
            config: ExtractConfig::default(),
            media: MediaInfo {
                width: 1,
                height: 1,
                duration_ms: Some(1000),
                codec: "fixture".into(),
                time_base_num: 1,
                time_base_den: 1000,
                subtitle_streams: 0,
                rotation: 0,
            },
            cues: vec![],
            processed_ms: 1000,
            active_appearances: vec![],
            completed: true,
            warnings: vec!["Final frame estimate".into()],
        };
        assert_eq!(super::project(&project), 0);
        let mut cue = Cue::new(1, 0, 0, 1000, "سلام 日本".into()).unwrap();
        cue.confidence = Some(1.0);
        project.cues.push(cue);
        assert_eq!(super::project(&project), 2);
        project.cues[0].reviewed = true;
        project.cues[0].confidence = None;
        assert_eq!(super::project(&project), 0);
    }

    #[test]
    fn failure_codes_follow_typed_origin_not_diagnostic_text() {
        assert_eq!(failure(&AppError::InvalidConfig("invalid".into())), 3);
        assert_eq!(failure(&AppError::Cancelled), 5);
        assert_eq!(failure(&AppError::Provider("missing".into())), 6);
        assert_eq!(failure(&nvsf_app::fail("Required provider unavailable")), 4);
        assert_eq!(
            failure(&AppError::Core(nvsf_core::Error::Invalid(
                "runtime cue".into()
            ))),
            4
        );
        assert_eq!(failure(&std::io::Error::other("missing input").into()), 4);
    }
}
