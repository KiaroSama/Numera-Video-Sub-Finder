use crate::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub schema: u32,
    pub enabled: bool,
    pub last_attempt: Option<u64>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            schema: 1,
            enabled: true,
            last_attempt: None,
        }
    }
}

impl Preferences {
    pub fn due(&self, now: u64, manual: bool) -> bool {
        manual
            || (self.enabled
                && self
                    .last_attempt
                    .is_none_or(|last| now.saturating_sub(last) >= 86_400))
    }
}

pub fn newer_stable(current: &str, offered: &str) -> Result<bool> {
    let parse = |value: &str| {
        semver::Version::parse(value.strip_prefix('v').unwrap_or(value))
            .map_err(|_| Error::Invalid("invalid application release version".into()))
    };
    let current = parse(current)?;
    let offered = parse(offered)?;
    Ok(offered.pre.is_empty() && offered.cmp_precedence(&current).is_gt())
}

pub fn authorize_install(consent: bool, busy: bool, unsaved: bool, supported: bool) -> Result<()> {
    if !consent {
        return Err(Error::Invalid(
            "explicit update installation approval required".into(),
        ));
    }
    if busy {
        return Err(Error::Invalid(
            "wait for active jobs and resource operations to finish".into(),
        ));
    }
    if unsaved {
        return Err(Error::Invalid(
            "save current review edits before installing an update".into(),
        ));
    }
    if !supported {
        return Err(Error::Invalid(
            "this package requires manual installation of the verified download".into(),
        ));
    }
    Ok(())
}
