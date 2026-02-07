pub mod error;
pub mod models;
pub mod traits;

pub use error::AppError;
pub use models::{Extraction, ExtractionSchema, compute_hash};
pub use traits::{Cleaner, Extractor, Fetcher};
