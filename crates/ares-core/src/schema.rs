use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::AppError;

/// A fully resolved schema: path, canonical name, and parsed JSON.
#[derive(Debug, Clone)]
pub struct ResolvedSchema {
    pub path: PathBuf,
    pub name: String,
    pub schema: serde_json::Value,
}

/// Resolves schema references (file paths or `name@version` strings) to
/// loaded JSON schemas.
pub struct SchemaResolver {
    schemas_dir: PathBuf,
}

impl SchemaResolver {
    pub fn new(schemas_dir: impl Into<PathBuf>) -> Self {
        Self {
            schemas_dir: schemas_dir.into(),
        }
    }

    /// Resolve a schema reference to a loaded [`ResolvedSchema`].
    ///
    /// Accepts:
    /// - A direct file path (e.g. `schemas/blog.json`)
    /// - `name@version` (e.g. `blog@1.0.0`)
    /// - `name@latest` (resolved via `registry.json`)
    pub fn resolve(&self, schema_ref: &str) -> Result<ResolvedSchema, AppError> {
        let (path, name) = self.resolve_path(schema_ref)?;

        let schema_str = std::fs::read_to_string(&path).map_err(|e| {
            AppError::SchemaError(format!(
                "Failed to read schema file {}: {e}",
                path.display()
            ))
        })?;

        let schema: serde_json::Value = serde_json::from_str(&schema_str).map_err(|e| {
            AppError::SchemaError(format!(
                "Invalid JSON in schema file {}: {e}",
                path.display()
            ))
        })?;

        Ok(ResolvedSchema { path, name, schema })
    }

    /// Resolve a schema reference to a `(path, name)` pair without reading the file.
    fn resolve_path(&self, schema_ref: &str) -> Result<(PathBuf, String), AppError> {
        // 1. Check if it's a direct file path.
        let path_candidate = PathBuf::from(schema_ref);
        if path_candidate.exists() {
            // Try structured name extraction: strip the schemas_dir prefix
            // and check for {name}/{version}.json structure.
            let name = self
                .structured_name(&path_candidate)
                .unwrap_or_else(|| derive_schema_name(&path_candidate));
            return Ok((path_candidate, name));
        }

        // 2. Parse name@version format.
        let (name, version) = schema_ref
            .split_once('@')
            .ok_or_else(|| AppError::SchemaError(format!("Schema not found: {schema_ref}")))?;
        if name.is_empty() || version.is_empty() {
            return Err(AppError::SchemaError(format!(
                "Schema must be in the form name@version, got: {schema_ref}"
            )));
        }

        // 3. Resolve @latest via the registry.
        let resolved_version = if version == "latest" {
            let registry = self.load_registry()?;
            registry.get(name).cloned().ok_or_else(|| {
                AppError::SchemaError(format!("No latest version for schema {name}"))
            })?
        } else {
            version.to_string()
        };

        // 4. Construct and validate the path.
        let schema_path = self
            .schemas_dir
            .join(name)
            .join(format!("{resolved_version}.json"));
        if !schema_path.exists() {
            return Err(AppError::SchemaError(format!(
                "Schema file not found: {}",
                schema_path.display()
            )));
        }

        Ok((schema_path, format!("{name}@{resolved_version}")))
    }

    /// Try to extract a `name@version` identifier by stripping `schemas_dir`
    /// and expecting `{name}/{version}.json` underneath.
    fn structured_name(&self, path: &Path) -> Option<String> {
        let abs_path = path.canonicalize().ok()?;
        let abs_dir = self.schemas_dir.canonicalize().ok()?;
        let relative = abs_path.strip_prefix(&abs_dir).ok()?;
        // Expect exactly 2 components: name/version.json
        let mut components = relative.components();
        let name = components.next()?.as_os_str().to_str()?;
        let file = components.next()?.as_os_str().to_str()?;
        // Must have no further components
        if components.next().is_some() {
            return None;
        }
        let version = Path::new(file).file_stem()?.to_str()?;
        Some(format!("{name}@{version}"))
    }

    /// Load and parse the schema registry (`registry.json`).
    fn load_registry(&self) -> Result<HashMap<String, String>, AppError> {
        let registry_path = self.schemas_dir.join("registry.json");
        let registry_str = std::fs::read_to_string(&registry_path).map_err(|e| {
            AppError::SchemaError(format!(
                "Failed to read schema registry {}: {e}",
                registry_path.display()
            ))
        })?;
        let registry: HashMap<String, String> = serde_json::from_str(&registry_str)
            .map_err(|e| AppError::SchemaError(format!("Invalid JSON in schema registry: {e}")))?;
        Ok(registry)
    }
}

/// Derive a schema name from a file path.
///
/// Extracts the file stem (name without extension).
/// Example: `"schemas/real_estate.json"` → `"real_estate"`
pub fn derive_schema_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("default")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_schema(dir: &Path, rel_path: &str, content: &str) {
        let full = dir.join(rel_path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, content).unwrap();
    }

    const SAMPLE_SCHEMA: &str = r#"{"type": "object"}"#;

    #[test]
    fn test_resolve_direct_path() {
        let tmp = TempDir::new().unwrap();
        let schema_file = tmp.path().join("my_schema.json");
        std::fs::write(&schema_file, SAMPLE_SCHEMA).unwrap();

        let resolver = SchemaResolver::new(tmp.path());
        let resolved = resolver.resolve(schema_file.to_str().unwrap()).unwrap();

        assert_eq!(resolved.name, "my_schema");
        assert_eq!(resolved.path, schema_file);
        assert_eq!(resolved.schema, serde_json::json!({"type": "object"}));
    }

    #[test]
    fn test_resolve_name_at_version() {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        write_schema(&schemas_dir, "blog/1.0.0.json", SAMPLE_SCHEMA);

        let resolver = SchemaResolver::new(&schemas_dir);
        let resolved = resolver.resolve("blog@1.0.0").unwrap();

        assert_eq!(resolved.name, "blog@1.0.0");
        assert_eq!(resolved.path, schemas_dir.join("blog/1.0.0.json"));
    }

    #[test]
    fn test_resolve_name_at_latest() {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        write_schema(&schemas_dir, "blog/2.0.0.json", SAMPLE_SCHEMA);
        write_schema(
            &schemas_dir,
            "../schemas/registry.json",
            r#"{"blog": "2.0.0"}"#,
        );

        let resolver = SchemaResolver::new(&schemas_dir);
        let resolved = resolver.resolve("blog@latest").unwrap();

        assert_eq!(resolved.name, "blog@2.0.0");
        assert_eq!(resolved.path, schemas_dir.join("blog/2.0.0.json"));
    }

    #[test]
    fn test_resolve_missing_schema() {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();

        let resolver = SchemaResolver::new(&schemas_dir);
        let err = resolver.resolve("missing@1.0.0").unwrap_err();

        assert!(matches!(err, AppError::SchemaError(_)));
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_resolve_invalid_format() {
        let tmp = TempDir::new().unwrap();
        let resolver = SchemaResolver::new(tmp.path());
        let err = resolver.resolve("no-at-sign").unwrap_err();

        assert!(matches!(err, AppError::SchemaError(_)));
    }

    #[test]
    fn test_resolve_empty_name_or_version() {
        let tmp = TempDir::new().unwrap();
        let resolver = SchemaResolver::new(tmp.path());

        let err = resolver.resolve("@1.0.0").unwrap_err();
        assert!(matches!(err, AppError::SchemaError(_)));

        let err = resolver.resolve("name@").unwrap_err();
        assert!(matches!(err, AppError::SchemaError(_)));
    }

    #[test]
    fn test_derive_schema_name() {
        assert_eq!(derive_schema_name(Path::new("schema.json")), "schema");
        assert_eq!(
            derive_schema_name(Path::new("schemas/real_estate.json")),
            "real_estate"
        );
        assert_eq!(
            derive_schema_name(Path::new("/absolute/path/to/my_schema.json")),
            "my_schema"
        );
    }

    #[test]
    fn test_derive_schema_name_no_extension() {
        assert_eq!(derive_schema_name(Path::new("schema")), "schema");
    }

    #[test]
    fn test_resolve_direct_path_inside_schemas_dir() {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        write_schema(&schemas_dir, "blog/1.0.0.json", SAMPLE_SCHEMA);

        let resolver = SchemaResolver::new(&schemas_dir);
        let schema_file = schemas_dir.join("blog/1.0.0.json");
        let resolved = resolver.resolve(schema_file.to_str().unwrap()).unwrap();

        // Should derive structured name because path is inside schemas_dir.
        assert_eq!(resolved.name, "blog@1.0.0");
    }

    #[test]
    fn test_structured_name() {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        write_schema(&schemas_dir, "blog/1.0.0.json", SAMPLE_SCHEMA);
        write_schema(&schemas_dir, "flat.json", SAMPLE_SCHEMA);

        let resolver = SchemaResolver::new(&schemas_dir);

        // Structured path → name@version
        let blog_path = schemas_dir.join("blog/1.0.0.json");
        assert_eq!(
            resolver.structured_name(&blog_path),
            Some("blog@1.0.0".to_string())
        );

        // Flat file directly in schemas_dir → None (not enough depth)
        let flat_path = schemas_dir.join("flat.json");
        assert_eq!(resolver.structured_name(&flat_path), None);
    }
}
