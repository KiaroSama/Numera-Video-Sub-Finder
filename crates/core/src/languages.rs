use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Language {
    pub code: String,
    pub name: String,
    pub url: String,
    pub bytes: u64,
    pub sha256: String,
    pub vertical: bool,
}

#[derive(Deserialize)]
struct Catalog {
    schema: u32,
    languages: Vec<Language>,
}

pub fn catalog() -> &'static [Language] {
    static CATALOG: OnceLock<Vec<Language>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let catalog: Catalog = serde_json::from_str(include_str!("../../../ci/languages.json"))
            .expect("embedded official language catalog must be valid");
        assert_eq!(catalog.schema, 1, "unsupported embedded language catalog");
        catalog.languages
    })
}

pub fn find(code: &str) -> Option<&'static Language> {
    let code = match code {
        "en" => "eng",
        "fa" => "fas",
        "ja" => "jpn",
        "fr" => "fra",
        "de" => "deu",
        "es" => "spa",
        "it" => "ita",
        "pt" => "por",
        "ru" => "rus",
        "ar" => "ara",
        "hi" => "hin",
        "ko" => "kor",
        "zh" | "zh-CN" => "chi_sim",
        "zh-TW" => "chi_tra",
        "ur" => "urd",
        "uk" => "ukr",
        "tr" => "tur",
        "vi" => "vie",
        _ => code,
    };
    catalog().iter().find(|language| language.code == code)
}

pub fn paddle_model(code: &str) -> Option<&'static str> {
    match find(code)?.code.as_str() {
        "eng" | "jpn" | "chi_sim" | "chi_tra" => Some("en-ja"),
        "fas" | "ara" | "urd" | "pus" | "uig" | "snd" => Some("fa"),
        _ => None,
    }
}

pub fn is_persian(code: &str) -> bool {
    find(code).is_some_and(|language| language.code == "fas")
}
