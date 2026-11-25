use crate::helpers::{LanguageOption, language_options};

const MAX_MATCHES: usize = 30;

pub(crate) fn current_language_option(code: &str) -> Option<LanguageOption> {
    if language_options("")
        .iter()
        .any(|option| option.code == code)
    {
        return None;
    }
    let name = nvsf_core::languages::find(code)
        .map_or("Current configuration", |language| language.name.as_str());
    Some(LanguageOption {
        code: code.to_owned(),
        label: format!("{name} ({code})"),
    })
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LanguageSearch {
    query: String,
    expanded: bool,
    active: Option<usize>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum KeyAction {
    Ignored,
    Handled,
    Selected(String),
}

impl LanguageSearch {
    pub(crate) fn query(&self) -> &str {
        &self.query
    }

    pub(crate) fn set_query(&mut self, query: String) {
        self.query = query;
        self.close();
        self.open();
    }

    pub(crate) fn is_open(&self) -> bool {
        self.expanded
    }

    pub(crate) fn options(&self) -> Vec<LanguageOption> {
        language_options(&self.query)
            .into_iter()
            .take(MAX_MATCHES)
            .collect()
    }

    pub(crate) fn active_code(&self) -> Option<String> {
        if !self.expanded {
            return None;
        }
        self.options().get(self.active?).map(|l| l.code.clone())
    }

    pub(crate) fn open(&mut self) {
        if !self.query.trim().is_empty() {
            self.expanded = true;
            self.active = (!self.options().is_empty()).then_some(0);
        }
    }

    pub(crate) fn close(&mut self) {
        self.expanded = false;
        self.active = None;
    }

    pub(crate) fn clear(&mut self) {
        self.query.clear();
        self.close();
    }

    pub(crate) fn choose(&mut self, code: &str) -> Option<String> {
        if !self.expanded || !self.options().iter().any(|l| l.code == code) {
            return None;
        }
        let choice = code.to_owned();
        self.clear();
        Some(choice)
    }

    pub(crate) fn key(&mut self, key: &str, composing: bool) -> KeyAction {
        if composing {
            return KeyAction::Ignored;
        }
        match key {
            "ArrowDown" | "ArrowUp" => {
                let length = self.options().len();
                self.expanded = true;
                self.active = if length == 0 {
                    None
                } else {
                    Some(match (self.active, key) {
                        (Some(index), "ArrowDown") => (index + 1).min(length - 1),
                        (Some(index), _) => index.saturating_sub(1),
                        (None, "ArrowDown") => 0,
                        (None, _) => length - 1,
                    })
                };
                KeyAction::Handled
            }
            "Enter" => self
                .active_code()
                .and_then(|code| self.choose(&code))
                .map_or(KeyAction::Ignored, KeyAction::Selected),
            "Escape" if self.expanded || !self.query.is_empty() => {
                self.clear();
                KeyAction::Handled
            }
            "Tab" => {
                self.close();
                KeyAction::Ignored
            }
            _ => KeyAction::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_alias_and_unknown_current_codes_remain_truthfully_represented() {
        let alias = current_language_option("fr").expect("legacy French alias");
        assert_eq!(alias.code, "fr");
        assert!(alias.label.contains("French"));
        assert!(current_language_option("fra").is_none());
        assert!(current_language_option("fa").is_none());
        let unknown = current_language_option("not-a-catalog-code").unwrap();
        assert_eq!(unknown.code, "not-a-catalog-code");
        assert_eq!(unknown.label, "Current configuration (not-a-catalog-code)");
    }

    #[test]
    fn searching_and_cancelling_never_choose_a_language() {
        let mut search = LanguageSearch::default();
        search.set_query("فارسی".into());
        assert!(search.is_open());
        assert_eq!(search.active_code().as_deref(), Some("fa"));
        assert_eq!(search.key("Escape", false), KeyAction::Handled);
        assert_eq!(search.query(), "");
        assert!(!search.is_open());
        assert_eq!(search.key("Enter", false), KeyAction::Ignored);

        search.set_query("zzzz-no-language".into());
        assert!(search.options().is_empty());
        assert_eq!(search.active_code(), None);
        assert_eq!(search.key("Enter", false), KeyAction::Ignored);
        assert_eq!(search.choose("en"), None);
        search.clear();
        assert_eq!(search.key("Enter", false), KeyAction::Ignored);
    }

    #[test]
    fn arrows_choose_the_announced_option_only_on_enter() {
        let mut search = LanguageSearch::default();
        search.set_query("chinese vertical".into());
        let options = search.options();
        assert_eq!(options.len(), 2);
        assert_eq!(search.active_code(), Some(options[0].code.clone()));
        assert_eq!(search.key("ArrowDown", false), KeyAction::Handled);
        assert_eq!(search.active_code(), Some(options[1].code.clone()));
        assert_eq!(search.key("ArrowDown", false), KeyAction::Handled);
        assert_eq!(search.active_code(), Some(options[1].code.clone()));
        assert_eq!(
            search.key("Enter", false),
            KeyAction::Selected(options[1].code.clone())
        );
        assert_eq!(search.query(), "");
        assert!(!search.is_open());
        assert_eq!(search.active_code(), None);
    }

    #[test]
    fn closed_popup_composition_and_text_keys_cannot_commit() {
        let mut search = LanguageSearch::default();
        search.set_query("japanese".into());
        let before = search.active_code();
        for key in ["Enter", "Escape", "ArrowDown"] {
            assert_eq!(search.key(key, true), KeyAction::Ignored);
            assert_eq!(search.active_code(), before);
        }
        for key in ["ArrowLeft", "ArrowRight", "Home", "End", "a"] {
            assert_eq!(search.key(key, false), KeyAction::Ignored);
        }
        assert_eq!(search.key("Tab", false), KeyAction::Ignored);
        assert!(!search.is_open());
        assert_eq!(search.active_code(), None);
        assert_eq!(search.query(), "japanese");
        assert_eq!(search.key("Enter", false), KeyAction::Ignored);
        search.open();
        assert_eq!(search.active_code(), before);
    }

    #[test]
    fn query_changes_reset_navigation_and_keep_every_catalog_language_reachable() {
        let mut search = LanguageSearch::default();
        assert_eq!(search.key("ArrowUp", false), KeyAction::Handled);
        assert_eq!(search.options().len(), MAX_MATCHES);
        assert_eq!(
            search.active_code(),
            search.options().last().map(|l| l.code.clone())
        );
        search.set_query("aze_cyrl".into());
        assert_eq!(search.active_code().as_deref(), Some("aze_cyrl"));
        assert_eq!(search.choose("fa"), None);
        assert_eq!(search.choose("aze_cyrl"), Some("aze_cyrl".into()));
        assert!(!search.is_open());

        let all = crate::helpers::language_options("");
        assert_eq!(all.len(), nvsf_core::languages::catalog().len() + 3);
        for language in all {
            search.set_query(language.code.clone());
            assert!(search.options().iter().any(|l| l.code == language.code));
        }
    }
}
