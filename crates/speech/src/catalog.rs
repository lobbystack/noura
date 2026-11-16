//! The speech-to-text models Noura offers: Handy's catalog from
//! `handy-computer/models`, compiled in, keeping only the models whose
//! license allows commercial use. Each entry names a GGUF file on Hugging
//! Face at a pinned revision, with its size and SHA-256, and the benchmark
//! results that grade it per language and time it.
//!
//! The grades and the recommendation follow Handy's Models page: a letter
//! per language from the error rate on FLEURS, and the wait after a
//! 30-second dictation from the CPU benchmark.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

/// The catalog file, as `handy-computer/models` publishes it.
const CATALOG_JSON: &str = include_str!("../catalog/catalog.json");

/// The model onboarding recommends.
pub const ONBOARDING_MODEL: &str = "parakeet-ultra";

/// The voice activity model Noura cuts audio with, for models that can't
/// stream.
pub const VAD_MODEL: &str = "silero-vad-v6.2";

/// Licenses that allow commercial use. A model under any other, such as
/// `cc-by-nc-4.0` or `other`, isn't offered.
const COMMERCIAL_LICENSES: [&str; 4] = ["mit", "apache-2.0", "cc-by-4.0", "cc-by-sa-4.0"];

/// The most errors in percent for each grade, as Handy grades them. More
/// is an F.
const GRADE_CEILINGS: [(Grade, f64); 4] = [
    (Grade::A, 5.0),
    (Grade::B, 10.0),
    (Grade::C, 20.0),
    (Grade::D, 30.0),
];

/// How well a model hears one language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Grade {
    A,
    B,
    C,
    D,
    F,
}

impl Grade {
    /// The grade for `error` percent of words wrong.
    pub fn for_error(error: f64) -> Self {
        GRADE_CEILINGS
            .iter()
            .find(|(_, ceiling)| error <= *ceiling)
            .map_or(Grade::F, |(grade, _)| *grade)
    }

    pub fn letter(self) -> &'static str {
        match self {
            Grade::A => "A",
            Grade::B => "B",
            Grade::C => "C",
            Grade::D => "D",
            Grade::F => "F",
        }
    }
}

/// A model file on Hugging Face.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFile {
    /// `owner/name` of the repository.
    pub repository: String,
    /// The commit the file is pinned at.
    pub revision: String,
    pub filename: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
}

/// One model Noura offers.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogModel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub license: String,
    /// ISO 639-1 codes.
    pub languages: Vec<String>,
    pub streaming: bool,
    pub translation: bool,
    /// Handy marks the models it shows by default.
    pub curated: bool,
    pub file: ModelFile,
    /// Percent of words wrong, by language.
    pub errors: BTreeMap<String, f64>,
    /// Times faster than real time on the benchmark's CPU.
    pub cpu_speed: Option<f64>,
}

impl CatalogModel {
    /// The grade for `language`, or `None` when the model doesn't speak it
    /// or the benchmark didn't measure it.
    pub fn grade(&self, language: &str) -> Option<Grade> {
        self.errors.get(language).copied().map(Grade::for_error)
    }

    /// Seconds Noura takes after 30 seconds of speech, on a CPU.
    pub fn wait_seconds(&self) -> Option<f64> {
        self.cpu_speed
            .filter(|speed| *speed > 0.0)
            .map(|speed| methodology().dictation_seconds / speed)
    }

    pub fn speaks(&self, language: &str) -> bool {
        self.languages.iter().any(|known| known == language)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCatalog {
    methodology: Methodology,
    models: Vec<RawModel>,
    #[serde(default)]
    results: BTreeMap<String, RawResults>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
struct Methodology {
    dictation_seconds: f64,
    overall_max_wait_seconds: f64,
    max_language_error_percent: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawModel {
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    repository: String,
    revision: String,
    license: String,
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default)]
    capabilities: RawCapabilities,
    quant: String,
    role: String,
    #[serde(default)]
    curated: bool,
    files: BTreeMap<String, RawFile>,
}

#[derive(Deserialize, Default)]
struct RawCapabilities {
    #[serde(default)]
    streaming: bool,
    #[serde(default)]
    translate: bool,
}

#[derive(Deserialize)]
struct RawFile {
    filename: String,
    size: u64,
    sha256: String,
}

#[derive(Deserialize, Default)]
struct RawResults {
    #[serde(default)]
    accuracy: BTreeMap<String, RawAccuracy>,
    #[serde(default)]
    performance: RawPerformance,
}

#[derive(Deserialize)]
struct RawAccuracy {
    error: f64,
}

#[derive(Deserialize, Default)]
struct RawPerformance {
    cpu: Option<f64>,
}

/// The whole catalog: the models Noura offers and the helpers, such as
/// the voice activity model.
pub struct Catalog {
    models: Vec<CatalogModel>,
    helpers: Vec<CatalogModel>,
    methodology: Methodology,
}

fn parsed() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| Catalog::parse(CATALOG_JSON).expect("the bundled catalog parses"))
}

fn methodology() -> Methodology {
    parsed().methodology
}

/// The bundled catalog.
pub fn catalog() -> &'static Catalog {
    parsed()
}

impl Catalog {
    /// Parse a catalog file in the format of `handy-computer/models`.
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        let raw: RawCatalog = serde_json::from_str(json)?;
        let mut models = Vec::new();
        let mut helpers = Vec::new();
        for model in raw.models {
            if !COMMERCIAL_LICENSES.contains(&model.license.as_str()) {
                continue;
            }
            let Some(file) = model.files.get(&model.quant) else {
                continue;
            };
            // Noura keeps models in one folder, so a file in a subfolder of
            // its repository isn't offered.
            if file.filename.contains('/') || file.filename.contains('\\') {
                continue;
            }
            let results = raw.results.get(&model.id);
            let converted = CatalogModel {
                file: ModelFile {
                    repository: model.repository.clone(),
                    revision: model.revision.clone(),
                    filename: file.filename.clone(),
                    size: file.size,
                    sha256: file.sha256.to_ascii_lowercase(),
                },
                id: model.id,
                name: model.name,
                description: model.description,
                license: model.license,
                languages: model.languages,
                streaming: model.capabilities.streaming,
                translation: model.capabilities.translate,
                curated: model.curated,
                errors: results
                    .map(|results| {
                        results
                            .accuracy
                            .iter()
                            .map(|(language, cell)| (language.clone(), cell.error))
                            .collect()
                    })
                    .unwrap_or_default(),
                cpu_speed: results.and_then(|results| results.performance.cpu),
            };
            match model.role.as_str() {
                "asr" => models.push(converted),
                "vad" => helpers.push(converted),
                _ => {}
            }
        }
        Ok(Self {
            models,
            helpers,
            methodology: raw.methodology,
        })
    }

    /// The models Noura offers, in the catalog's order.
    pub fn models(&self) -> &[CatalogModel] {
        &self.models
    }

    pub fn model(&self, id: &str) -> Option<&CatalogModel> {
        self.models.iter().find(|model| model.id == id)
    }

    /// A helper model, such as [`VAD_MODEL`].
    pub fn helper(&self, id: &str) -> Option<&CatalogModel> {
        self.helpers.iter().find(|model| model.id == id)
    }

    /// The model Noura recommends for someone who speaks `languages`: the
    /// curated model with the fewest errors across them, among those that
    /// hear each one with under 20% errors and answer a 30-second
    /// dictation within 5 seconds on a CPU. Handy's Models page picks the
    /// same way.
    pub fn recommended(&self, languages: &[String]) -> Option<&CatalogModel> {
        let methodology = self.methodology;
        let fast = |model: &&CatalogModel| {
            model.cpu_speed.is_some_and(|speed| {
                methodology.dictation_seconds / speed <= methodology.overall_max_wait_seconds
            })
        };
        let score = |model: &CatalogModel| -> Option<f64> {
            let errors: Option<Vec<f64>> = languages
                .iter()
                .map(|language| model.errors.get(language).copied())
                .collect();
            let errors = errors?;
            if errors.is_empty()
                || errors
                    .iter()
                    .any(|error| *error >= methodology.max_language_error_percent || *error <= 0.0)
            {
                return None;
            }
            // The geometric mean, as Handy averages several languages.
            Some((errors.iter().map(|error| error.ln()).sum::<f64>() / errors.len() as f64).exp())
        };
        self.models
            .iter()
            .filter(|model| model.curated)
            .filter(fast)
            .filter_map(|model| score(model).map(|score| (model, score)))
            .min_by(|left, right| left.1.total_cmp(&right.1))
            .map(|(model, _)| model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_models_licensed_for_commercial_use_are_offered() {
        let catalog = catalog();
        assert!(!catalog.models().is_empty());
        for model in catalog.models() {
            assert!(
                COMMERCIAL_LICENSES.contains(&model.license.as_str()),
                "{} is {}",
                model.id,
                model.license
            );
            assert_eq!(model.file.sha256.len(), 64, "{}", model.id);
        }
        for refused in [
            "canary-1b",
            "nemotron-3.5-asr-streaming-0.6b",
            "granite-speech-5.0-470m-turboctc-nc",
        ] {
            assert!(catalog.model(refused).is_none(), "{refused}");
        }
        assert!(catalog.helper(VAD_MODEL).is_some());
    }

    #[test]
    fn onboarding_recommends_parakeet_ultra_which_allows_commercial_use() {
        let ultra = catalog().model(ONBOARDING_MODEL).expect("Parakeet Ultra");
        assert_eq!(ultra.name, "Parakeet Ultra");
        assert_eq!(ultra.license, "cc-by-4.0");
        assert_eq!(ultra.grade("en"), Some(Grade::A));
        assert!(ultra.speaks("fr"));
        assert!(!ultra.streaming);
        assert!(ultra.wait_seconds().is_some_and(|wait| wait < 5.0));
    }

    #[test]
    fn grades_follow_handys_ceilings() {
        assert_eq!(Grade::for_error(4.9), Grade::A);
        assert_eq!(Grade::for_error(10.0), Grade::B);
        assert_eq!(Grade::for_error(19.0), Grade::C);
        assert_eq!(Grade::for_error(29.0), Grade::D);
        assert_eq!(Grade::for_error(31.0), Grade::F);
    }

    #[test]
    fn the_recommendation_hears_every_language_you_speak() {
        let catalog = catalog();
        let french = catalog
            .recommended(&["fr".to_owned()])
            .expect("a French model");
        assert!(french.speaks("fr"));
        let both = catalog
            .recommended(&["en".to_owned(), "de".to_owned()])
            .expect("a model for English and German");
        assert!(both.speaks("en") && both.speaks("de"));
        assert!(catalog.recommended(&["xx".to_owned()]).is_none());
    }
}
