//! Feature-gated native CPU inference using Candle and a pinned Qwen2.5 GGUF.
//!
//! The public model-management API deliberately supports one artifact only. That
//! keeps loading, prompt formatting, and benchmark results reproducible.

use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use candle_core::quantized::gguf_file;
use candle_core::{Device, Tensor};
use candle_transformers::generation::LogitsProcessor;
use candle_transformers::models::quantized_qwen2::ModelWeights;
use directories::BaseDirs;
use hf_hub::api::sync::ApiBuilder;
use hf_hub::{Cache, Repo, RepoType};
use serde::{Deserialize, Serialize};
use tokenizers::Tokenizer;

use ares_core::error::AppError;
use ares_core::schema::validate_extracted_output;
use ares_core::traits::{Extractor, ExtractorFactory};

use crate::{LOCAL_MODEL_ALIAS, util::truncate_for_error};

const WEIGHTS_REPO: &str = "Qwen/Qwen2.5-3B-Instruct-GGUF";
const WEIGHTS_REVISION: &str = "7dabda4d13d513e3e842b20f0d435c732f172cbe";
const WEIGHTS_FILE: &str = "qwen2.5-3b-instruct-q4_k_m.gguf";
const TOKENIZER_REPO: &str = "Qwen/Qwen2.5-3B-Instruct";
const TOKENIZER_REVISION: &str = "aa8e72537993ba99e69dfaafa59ed015b17504d1";
const TOKENIZER_FILES: &[&str] = &[
    "config.json",
    "generation_config.json",
    "tokenizer.json",
    "tokenizer_config.json",
];
const MAX_NEW_TOKENS: usize = 1_024;
const MAX_INPUT_TOKENS: usize = 7_168;
const DEFAULT_SYSTEM_PROMPT: &str = "You are a data extraction assistant. Extract the requested fields from the provided web content. Respond ONLY with valid JSON matching the requested schema. Do not include explanations.";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelManifest {
    alias: String,
    weights_repo: String,
    weights_revision: String,
    weights_file: String,
    tokenizer_repo: String,
    tokenizer_revision: String,
}

#[derive(Debug, Clone)]
pub struct LocalModelStatus {
    pub alias: String,
    pub bytes: u64,
    pub weights_revision: String,
    pub path: PathBuf,
}

/// A cache manager for the single pinned native model.
#[derive(Debug, Clone)]
pub struct LocalModelStore {
    root: PathBuf,
}

impl LocalModelStore {
    pub fn from_env() -> Result<Self, AppError> {
        let root = match std::env::var_os("ARES_MODEL_DIR") {
            Some(path) => PathBuf::from(path),
            None => BaseDirs::new()
                .map(|dirs| dirs.cache_dir().join("ares").join("models"))
                .ok_or_else(|| {
                    AppError::ConfigError(
                        "Could not determine a model cache directory; set ARES_MODEL_DIR".into(),
                    )
                })?,
        };
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn pull(&self, alias: &str) -> Result<LocalModelStatus, AppError> {
        validate_alias(alias)?;
        let model_dir = self.model_dir(alias);
        fs::create_dir_all(&model_dir).map_err(io_error)?;

        let api = ApiBuilder::new()
            .with_cache_dir(model_dir.join("hf"))
            .with_progress(true)
            .build()
            .map_err(|e| local_error(format!("Could not create Hugging Face client: {e}"), true))?;

        let weights = api.repo(weights_repo());
        weights
            .get(WEIGHTS_FILE)
            .map_err(|e| local_error(format!("Could not download {WEIGHTS_FILE}: {e}"), true))?;

        let tokenizer = api.repo(tokenizer_repo());
        for file in TOKENIZER_FILES {
            tokenizer.get(file).map_err(|e| {
                local_error(
                    format!("Could not download tokenizer file {file}: {e}"),
                    true,
                )
            })?;
        }

        let manifest = ModelManifest {
            alias: alias.to_string(),
            weights_repo: WEIGHTS_REPO.to_string(),
            weights_revision: WEIGHTS_REVISION.to_string(),
            weights_file: WEIGHTS_FILE.to_string(),
            tokenizer_repo: TOKENIZER_REPO.to_string(),
            tokenizer_revision: TOKENIZER_REVISION.to_string(),
        };
        write_manifest(&model_dir, &manifest)?;
        self.status(alias)?.ok_or_else(|| {
            local_error(
                "Model download completed without a readable cache entry".into(),
                false,
            )
        })
    }

    pub fn status(&self, alias: &str) -> Result<Option<LocalModelStatus>, AppError> {
        validate_alias(alias)?;
        let model_dir = self.model_dir(alias);
        let manifest_path = model_dir.join("manifest.json");
        if !manifest_path.exists() || !self.cached_files_exist(&model_dir) {
            return Ok(None);
        }
        let manifest: ModelManifest =
            serde_json::from_slice(&fs::read(&manifest_path).map_err(io_error)?)?;
        Ok(Some(LocalModelStatus {
            alias: manifest.alias,
            bytes: directory_size(&model_dir)?,
            weights_revision: manifest.weights_revision,
            path: model_dir,
        }))
    }

    pub fn list(&self) -> Result<Vec<LocalModelStatus>, AppError> {
        match self.status(LOCAL_MODEL_ALIAS)? {
            Some(status) => Ok(vec![status]),
            None => Ok(vec![]),
        }
    }

    pub fn remove(&self, alias: &str) -> Result<bool, AppError> {
        validate_alias(alias)?;
        let model_dir = self.model_dir(alias);
        if !model_dir.exists() {
            return Ok(false);
        }
        fs::remove_dir_all(model_dir).map_err(io_error)?;
        Ok(true)
    }

    fn paths(&self, alias: &str) -> Result<(PathBuf, PathBuf), AppError> {
        validate_alias(alias)?;
        let model_dir = self.model_dir(alias);
        if self.status(alias)?.is_none() {
            return Err(AppError::ConfigError(format!(
                "Local model '{alias}' is not cached. Run: ares model pull {alias}"
            )));
        }
        let cache = Cache::new(model_dir.join("hf"));
        let weights = cache
            .repo(weights_repo())
            .get(WEIGHTS_FILE)
            .ok_or_else(|| {
                AppError::ConfigError(
                    "Cached model weights are incomplete; pull the model again".into(),
                )
            })?;
        let tokenizer = cache
            .repo(tokenizer_repo())
            .get("tokenizer.json")
            .ok_or_else(|| {
                AppError::ConfigError("Cached tokenizer is incomplete; pull the model again".into())
            })?;
        Ok((weights, tokenizer))
    }

    fn cached_files_exist(&self, model_dir: &Path) -> bool {
        let cache = Cache::new(model_dir.join("hf"));
        cache.repo(weights_repo()).get(WEIGHTS_FILE).is_some()
            && TOKENIZER_FILES
                .iter()
                .all(|file| cache.repo(tokenizer_repo()).get(file).is_some())
    }

    fn model_dir(&self, alias: &str) -> PathBuf {
        self.root.join(alias)
    }
}

/// A loaded Qwen2 GGUF. It is guarded by a mutex because generation mutates
/// the model's KV cache.
struct LoadedModel {
    model: ModelWeights,
    tokenizer: Tokenizer,
    end_of_turn: Option<u32>,
}

type SharedModel = Arc<Mutex<LoadedModel>>;
static LOADED_MODELS: OnceLock<Mutex<HashMap<String, SharedModel>>> = OnceLock::new();

fn shared_model(store: &LocalModelStore, alias: &str) -> Result<SharedModel, AppError> {
    let models = LOADED_MODELS.get_or_init(|| Mutex::new(HashMap::new()));
    let models = models
        .lock()
        .map_err(|_| local_error("Local model cache lock was poisoned".into(), true))?;
    if let Some(model) = models.get(alias) {
        return Ok(model.clone());
    }

    drop(models);

    let loaded = LoadedModel::load(store, alias)?;
    let model = Arc::new(Mutex::new(loaded));

    let mut models = LOADED_MODELS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| local_error("Local model cache lock was poisoned".into(), true))?;
    if let Some(existing) = models.get(alias) {
        return Ok(existing.clone());
    }

    models.insert(alias.to_string(), model.clone());
    Ok(model)
}

impl LoadedModel {
    fn load(store: &LocalModelStore, alias: &str) -> Result<Self, AppError> {
        let (weights_path, tokenizer_path) = store.paths(alias)?;
        let device = Device::Cpu;
        let mut file = File::open(&weights_path).map_err(io_error)?;
        let content = gguf_file::Content::read(&mut file)
            .map_err(|e| local_error(format!("Could not read GGUF model: {e}"), true))?;
        let model = ModelWeights::from_gguf(content, &mut file, &device)
            .map_err(|e| local_error(format!("Could not load GGUF model: {e}"), true))?;
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| local_error(format!("Could not load tokenizer: {e}"), false))?;
        let end_of_turn = tokenizer.token_to_id("<|im_end|>");
        Ok(Self {
            model,
            tokenizer,
            end_of_turn,
        })
    }

    fn generate(&mut self, prompt: &str) -> Result<String, AppError> {
        let encoding = self
            .tokenizer
            .encode(prompt, false)
            .map_err(|e| local_error(format!("Could not tokenize prompt: {e}"), false))?;
        let tokens = encoding.get_ids();
        if tokens.len() > MAX_INPUT_TOKENS {
            return Err(AppError::InvalidInput(format!(
                "Local prompt is {} tokens; maximum is {MAX_INPUT_TOKENS}. Use --max-content to reduce page content.",
                tokens.len()
            )));
        }

        let device = Device::Cpu;
        let mut logits = self
            .model
            .forward(
                &Tensor::new(tokens, &device)
                    .map_err(candle_error)?
                    .unsqueeze(0)
                    .map_err(candle_error)?,
                0,
            )
            .map_err(candle_error)?
            .squeeze(0)
            .map_err(candle_error)?;
        let mut sampler = LogitsProcessor::new(0, None, None);
        let mut generated = Vec::with_capacity(MAX_NEW_TOKENS);

        for index in 0..MAX_NEW_TOKENS {
            let next = sampler.sample(&logits).map_err(candle_error)?;
            if Some(next) == self.end_of_turn {
                break;
            }
            generated.push(next);
            logits = self
                .model
                .forward(
                    &Tensor::new(&[next], &device)
                        .map_err(candle_error)?
                        .unsqueeze(0)
                        .map_err(candle_error)?,
                    tokens.len() + index,
                )
                .map_err(candle_error)?
                .squeeze(0)
                .map_err(candle_error)?;
        }

        self.tokenizer
            .decode(&generated, true)
            .map_err(|e| local_error(format!("Could not decode model output: {e}"), true))
    }
}

#[derive(Clone)]
pub struct CandleExtractor {
    model: SharedModel,
    system_prompt: String,
}

impl CandleExtractor {
    pub fn new(alias: &str) -> Result<Self, AppError> {
        let store = LocalModelStore::from_env()?;
        Ok(Self {
            model: shared_model(&store, alias)?,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
        })
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }
}

// The retry implementation below intentionally keeps the schema by reference;
// this helper separates output parsing from generation so the retry path can
// preserve the original schema exactly.
impl CandleExtractor {
    fn extract_once(
        &self,
        content: &str,
        schema: &serde_json::Value,
        correction: Option<&str>,
    ) -> Result<serde_json::Value, AppError> {
        let schema_text = serde_json::to_string_pretty(schema)?;
        let correction = correction.map(|message| format!("\n\nYour previous output was rejected: {message}. Return a corrected JSON object only.")).unwrap_or_default();
        let prompt = format!(
            "<|im_start|>system\n{}<|im_end|>\n<|im_start|>user\nExtract data according to this JSON schema:\n```json\n{schema_text}\n```\n\nFrom the following web content:\n\n{content}{correction}<|im_end|>\n<|im_start|>assistant\n",
            self.system_prompt,
        );
        let raw = self
            .model
            .lock()
            .map_err(|_| local_error("Local model lock was poisoned".into(), true))?
            .generate(&prompt)?;
        let value = serde_json::from_str(&raw).map_err(|e| {
            AppError::SchemaValidationError(format!(
                "Local model returned invalid JSON: {e}. Raw: {}",
                truncate_for_error(&raw)
            ))
        })?;
        validate_extracted_output(schema, &value)?;
        Ok(value)
    }
}

impl Extractor for CandleExtractor {
    async fn extract(
        &self,
        content: &str,
        schema: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        match self.extract_once(content, schema, None) {
            Ok(value) => Ok(value),
            Err(first_error) => {
                tracing::warn!(error = %first_error, "local extraction failed validation; retrying once");
                self.extract_once(content, schema, Some(&first_error.to_string()))
            }
        }
    }
}

#[derive(Clone)]
pub struct CandleExtractorFactory {
    store: LocalModelStore,
    system_prompt: Option<String>,
}

impl CandleExtractorFactory {
    pub fn new() -> Result<Self, AppError> {
        Ok(Self {
            store: LocalModelStore::from_env()?,
            system_prompt: None,
        })
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }
}

impl ExtractorFactory for CandleExtractorFactory {
    type Extractor = CandleExtractor;

    fn create(&self, model: &str, _base_url: &str) -> Result<Self::Extractor, AppError> {
        validate_alias(model)?;
        let model = shared_model(&self.store, model)?;
        let extractor = CandleExtractor {
            model,
            system_prompt: self
                .system_prompt
                .clone()
                .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string()),
        };
        Ok(extractor)
    }
}

fn validate_alias(alias: &str) -> Result<(), AppError> {
    if alias == LOCAL_MODEL_ALIAS {
        Ok(())
    } else {
        Err(AppError::InvalidInput(format!(
            "Unsupported local model '{alias}'. Supported models: {LOCAL_MODEL_ALIAS}"
        )))
    }
}

fn weights_repo() -> Repo {
    Repo::with_revision(
        WEIGHTS_REPO.to_string(),
        RepoType::Model,
        WEIGHTS_REVISION.to_string(),
    )
}

fn tokenizer_repo() -> Repo {
    Repo::with_revision(
        TOKENIZER_REPO.to_string(),
        RepoType::Model,
        TOKENIZER_REVISION.to_string(),
    )
}

fn write_manifest(model_dir: &Path, manifest: &ModelManifest) -> Result<(), AppError> {
    let bytes = serde_json::to_vec_pretty(manifest)?;
    let temporary = model_dir.join("manifest.json.part");
    fs::write(&temporary, bytes).map_err(io_error)?;
    let target = model_dir.join("manifest.json");
    if target.exists() {
        fs::remove_file(&temporary).map_err(io_error)?;
        return Ok(());
    }
    fs::rename(temporary, target).map_err(io_error)
}

fn directory_size(path: &Path) -> Result<u64, AppError> {
    let mut bytes = 0;
    for entry in fs::read_dir(path).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let metadata = entry.metadata().map_err(io_error)?;
        bytes += if metadata.is_dir() {
            directory_size(&entry.path())?
        } else {
            metadata.len()
        };
    }
    Ok(bytes)
}

fn io_error(error: std::io::Error) -> AppError {
    AppError::LocalInferenceError {
        message: error.to_string(),
        retryable: false,
    }
}

fn candle_error(error: candle_core::Error) -> AppError {
    local_error(error.to_string(), true)
}

fn local_error(message: String, retryable: bool) -> AppError {
    AppError::LocalInferenceError { message, retryable }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HtmdCleaner;
    use ares_core::traits::Cleaner;

    #[test]
    fn only_the_pinned_model_alias_is_accepted() {
        assert!(validate_alias(LOCAL_MODEL_ALIAS).is_ok());
        assert!(matches!(
            validate_alias("qwen2.5:3b"),
            Err(AppError::InvalidInput(_))
        ));
    }

    #[test]
    fn manifest_write_is_completed_without_a_part_file() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = ModelManifest {
            alias: LOCAL_MODEL_ALIAS.to_string(),
            weights_repo: WEIGHTS_REPO.to_string(),
            weights_revision: WEIGHTS_REVISION.to_string(),
            weights_file: WEIGHTS_FILE.to_string(),
            tokenizer_repo: TOKENIZER_REPO.to_string(),
            tokenizer_revision: TOKENIZER_REVISION.to_string(),
        };
        write_manifest(temp.path(), &manifest).unwrap();

        assert!(temp.path().join("manifest.json").exists());
        assert!(!temp.path().join("manifest.json.part").exists());
        let saved: ModelManifest =
            serde_json::from_slice(&fs::read(temp.path().join("manifest.json")).unwrap()).unwrap();
        assert_eq!(saved.weights_revision, WEIGHTS_REVISION);
    }

    #[test]
    fn missing_model_reports_a_pull_command() {
        let temp = tempfile::tempdir().unwrap();
        let store = LocalModelStore {
            root: temp.path().to_path_buf(),
        };
        let error = store.paths(LOCAL_MODEL_ALIAS).unwrap_err();
        assert!(error.to_string().contains("ares model pull"));
    }

    #[tokio::test]
    #[ignore = "requires the pinned Qwen model downloaded with `ares model pull`"]
    async fn pinned_model_extracts_blog_and_public_tender_fixtures() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let extractor = CandleExtractor::new(LOCAL_MODEL_ALIAS).unwrap();
        let cleaner = HtmdCleaner::new();

        for (fixture, schema) in [
            ("bench/fixtures/blog.html", "schemas/blog/1.0.0.json"),
            (
                "bench/fixtures/public_tender.html",
                "schemas/public_tenders/1.0.0.json",
            ),
        ] {
            let html = fs::read_to_string(root.join(fixture)).unwrap();
            let schema: serde_json::Value =
                serde_json::from_slice(&fs::read(root.join(schema)).unwrap()).unwrap();
            let value = extractor
                .extract(&cleaner.clean(&html).unwrap(), &schema)
                .await
                .unwrap();
            validate_extracted_output(&schema, &value).unwrap();
        }
    }
}
