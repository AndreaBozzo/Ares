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

/// A schema entry returned when listing schemas.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SchemaEntry {
    pub name: String,
    pub latest_version: String,
    pub versions: Vec<String>,
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
    pub fn load_registry(&self) -> Result<HashMap<String, String>, AppError> {
        let registry_path = self.schemas_dir.join("registry.json");
        if !registry_path.exists() {
            return Ok(HashMap::new());
        }
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

    /// List all schemas with their versions.
    pub fn list_schemas(&self) -> Result<Vec<SchemaEntry>, AppError> {
        let registry = self.load_registry()?;
        let mut entries = Vec::new();

        for (name, latest_version) in &registry {
            let versions = self.list_versions(name)?;
            entries.push(SchemaEntry {
                name: name.clone(),
                latest_version: latest_version.clone(),
                versions,
            });
        }

        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    /// List all version files for a given schema name.
    fn list_versions(&self, name: &str) -> Result<Vec<String>, AppError> {
        let schema_dir = self.schemas_dir.join(name);
        if !schema_dir.is_dir() {
            return Ok(vec![]);
        }

        let mut versions = Vec::new();
        let entries = std::fs::read_dir(&schema_dir).map_err(|e| {
            AppError::SchemaError(format!(
                "Failed to read schema directory {}: {e}",
                schema_dir.display()
            ))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                AppError::SchemaError(format!("Failed to read directory entry: {e}"))
            })?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    versions.push(stem.to_string());
                }
            }
        }

        versions.sort();
        Ok(versions)
    }

    /// Update the content of an existing schema version.
    ///
    /// Returns an error if the schema does not exist.
    /// Does not modify the registry — only the file content changes.
    pub fn update_schema(
        &self,
        name: &str,
        version: &str,
        schema: &serde_json::Value,
    ) -> Result<(), AppError> {
        if name.is_empty() || version.is_empty() {
            return Err(AppError::SchemaError(
                "Schema name and version must not be empty".to_string(),
            ));
        }

        let schema_path = self.schemas_dir.join(name).join(format!("{version}.json"));
        if !schema_path.exists() {
            return Err(AppError::SchemaError(format!(
                "Schema not found: {name}@{version}"
            )));
        }

        let pretty = serde_json::to_string_pretty(schema)
            .map_err(|e| AppError::SchemaError(e.to_string()))?;
        std::fs::write(&schema_path, pretty).map_err(|e| {
            AppError::SchemaError(format!(
                "Failed to write schema file {}: {e}",
                schema_path.display()
            ))
        })?;

        Ok(())
    }

    /// Delete a specific schema version file and update the registry.
    ///
    /// If the deleted version was the latest, the registry is updated to point
    /// to the next most recent version. If it was the only version, the entry
    /// is removed from the registry entirely.
    pub fn delete_schema(&self, name: &str, version: &str) -> Result<(), AppError> {
        if name.is_empty() || version.is_empty() {
            return Err(AppError::SchemaError(
                "Schema name and version must not be empty".to_string(),
            ));
        }

        let schema_path = self.schemas_dir.join(name).join(format!("{version}.json"));
        if !schema_path.exists() {
            return Err(AppError::SchemaError(format!(
                "Schema not found: {name}@{version}"
            )));
        }

        std::fs::remove_file(&schema_path).map_err(|e| {
            AppError::SchemaError(format!(
                "Failed to delete schema file {}: {e}",
                schema_path.display()
            ))
        })?;

        // Update the registry
        let mut registry = self.load_registry()?;
        let remaining = self.list_versions(name)?;

        if remaining.is_empty() {
            registry.remove(name);
        } else if registry.get(name).is_some_and(|latest| latest == version) {
            // Deleted version was the latest — point to the highest remaining
            registry.insert(name.to_string(), remaining.last().unwrap().clone());
        }

        let registry_path = self.schemas_dir.join("registry.json");
        let registry_json = serde_json::to_string_pretty(&registry)
            .map_err(|e| AppError::SchemaError(e.to_string()))?;
        std::fs::write(&registry_path, format!("{registry_json}\n")).map_err(|e| {
            AppError::SchemaError(format!(
                "Failed to write schema registry {}: {e}",
                registry_path.display()
            ))
        })?;

        // Clean up empty directory (non-fatal)
        let _ = std::fs::remove_dir(self.schemas_dir.join(name));

        Ok(())
    }

    /// Create a new schema version, writing the file and updating the registry.
    pub fn create_schema(
        &self,
        name: &str,
        version: &str,
        schema: &serde_json::Value,
    ) -> Result<(), AppError> {
        // Validate inputs
        if name.is_empty() || version.is_empty() {
            return Err(AppError::SchemaError(
                "Schema name and version must not be empty".to_string(),
            ));
        }

        // Create directory if needed
        let schema_dir = self.schemas_dir.join(name);
        std::fs::create_dir_all(&schema_dir).map_err(|e| {
            AppError::SchemaError(format!(
                "Failed to create schema directory {}: {e}",
                schema_dir.display()
            ))
        })?;

        // Write schema file
        let schema_path = schema_dir.join(format!("{version}.json"));
        let pretty = serde_json::to_string_pretty(schema)
            .map_err(|e| AppError::SchemaError(e.to_string()))?;
        std::fs::write(&schema_path, pretty).map_err(|e| {
            AppError::SchemaError(format!(
                "Failed to write schema file {}: {e}",
                schema_path.display()
            ))
        })?;

        // Update registry — only advance latest when the new version is higher
        let mut registry = self.load_registry()?;
        let should_update = registry
            .get(name)
            .is_none_or(|current| version_gt(version, current));
        if should_update {
            registry.insert(name.to_string(), version.to_string());
        }
        let registry_path = self.schemas_dir.join("registry.json");
        let registry_json = serde_json::to_string_pretty(&registry)
            .map_err(|e| AppError::SchemaError(e.to_string()))?;
        std::fs::write(&registry_path, format!("{registry_json}\n")).map_err(|e| {
            AppError::SchemaError(format!(
                "Failed to write schema registry {}: {e}",
                registry_path.display()
            ))
        })?;

        Ok(())
    }
}

/// Compare two dot-separated version strings (e.g. "1.2.3" > "1.1.0").
///
/// Returns `true` if `a` is strictly greater than `b`.
/// Non-numeric segments are compared lexicographically as a fallback.
fn version_gt(a: &str, b: &str) -> bool {
    let mut a_parts = a.split('.');
    let mut b_parts = b.split('.');

    loop {
        match (a_parts.next(), b_parts.next()) {
            (Some(ap), Some(bp)) => {
                let cmp = match (ap.parse::<u64>(), bp.parse::<u64>()) {
                    (Ok(an), Ok(bn)) => an.cmp(&bn),
                    _ => ap.cmp(bp),
                };
                match cmp {
                    std::cmp::Ordering::Greater => return true,
                    std::cmp::Ordering::Less => return false,
                    std::cmp::Ordering::Equal => continue,
                }
            }
            (Some(_), None) => return true,  // a has more segments
            (None, Some(_)) => return false, // b has more segments
            (None, None) => return false,    // equal
        }
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

    #[test]
    fn test_create_schema_writes_file_and_registry() {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();

        let resolver = SchemaResolver::new(&schemas_dir);
        let schema =
            serde_json::json!({"type": "object", "properties": {"title": {"type": "string"}}});

        resolver.create_schema("blog", "1.0.0", &schema).unwrap();

        // Verify file was written
        let file_path = schemas_dir.join("blog/1.0.0.json");
        assert!(file_path.exists());
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file_path).unwrap()).unwrap();
        assert_eq!(content, schema);

        // Verify registry was updated
        let registry = resolver.load_registry().unwrap();
        assert_eq!(registry.get("blog").unwrap(), "1.0.0");
    }

    #[test]
    fn test_create_schema_empty_name_errors() {
        let tmp = TempDir::new().unwrap();
        let resolver = SchemaResolver::new(tmp.path());
        let schema = serde_json::json!({"type": "object"});

        let err = resolver.create_schema("", "1.0.0", &schema).unwrap_err();
        assert!(matches!(err, AppError::SchemaError(_)));

        let err = resolver.create_schema("blog", "", &schema).unwrap_err();
        assert!(matches!(err, AppError::SchemaError(_)));
    }

    #[test]
    fn test_list_schemas_multiple() {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();

        let resolver = SchemaResolver::new(&schemas_dir);
        let schema = serde_json::json!({"type": "object"});

        resolver.create_schema("blog", "1.0.0", &schema).unwrap();
        resolver.create_schema("product", "2.0.0", &schema).unwrap();

        let entries = resolver.list_schemas().unwrap();
        assert_eq!(entries.len(), 2);
        // Should be sorted alphabetically
        assert_eq!(entries[0].name, "blog");
        assert_eq!(entries[1].name, "product");
        assert_eq!(entries[0].latest_version, "1.0.0");
        assert_eq!(entries[1].latest_version, "2.0.0");
    }

    #[test]
    fn test_list_versions_multiple() {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();

        let resolver = SchemaResolver::new(&schemas_dir);
        let schema = serde_json::json!({"type": "object"});

        resolver.create_schema("blog", "1.0.0", &schema).unwrap();
        resolver.create_schema("blog", "2.0.0", &schema).unwrap();
        resolver.create_schema("blog", "1.1.0", &schema).unwrap();

        let entries = resolver.list_schemas().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "blog");
        // Latest should be the highest version, not the last created
        assert_eq!(entries[0].latest_version, "2.0.0");
        // Versions should be sorted
        assert_eq!(entries[0].versions, vec!["1.0.0", "1.1.0", "2.0.0"]);
    }

    #[test]
    fn test_update_schema_overwrites_content() {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();

        let resolver = SchemaResolver::new(&schemas_dir);
        let original = serde_json::json!({"type": "object"});
        resolver.create_schema("blog", "1.0.0", &original).unwrap();

        let updated =
            serde_json::json!({"type": "object", "properties": {"title": {"type": "string"}}});
        resolver.update_schema("blog", "1.0.0", &updated).unwrap();

        let resolved = resolver.resolve("blog@1.0.0").unwrap();
        assert_eq!(resolved.schema, updated);
    }

    #[test]
    fn test_update_schema_not_found() {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();

        let resolver = SchemaResolver::new(&schemas_dir);
        let err = resolver
            .update_schema("missing", "1.0.0", &serde_json::json!({}))
            .unwrap_err();

        assert!(matches!(err, AppError::SchemaError(_)));
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_update_schema_empty_name_errors() {
        let tmp = TempDir::new().unwrap();
        let resolver = SchemaResolver::new(tmp.path());
        let schema = serde_json::json!({"type": "object"});

        let err = resolver.update_schema("", "1.0.0", &schema).unwrap_err();
        assert!(matches!(err, AppError::SchemaError(_)));

        let err = resolver.update_schema("blog", "", &schema).unwrap_err();
        assert!(matches!(err, AppError::SchemaError(_)));
    }

    #[test]
    fn test_update_schema_does_not_change_registry() {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();

        let resolver = SchemaResolver::new(&schemas_dir);
        let schema = serde_json::json!({"type": "object"});

        resolver.create_schema("blog", "1.0.0", &schema).unwrap();
        resolver.create_schema("blog", "2.0.0", &schema).unwrap();

        // Update v1.0.0 — registry should still point to 2.0.0
        let updated = serde_json::json!({"type": "array"});
        resolver.update_schema("blog", "1.0.0", &updated).unwrap();

        let registry = resolver.load_registry().unwrap();
        assert_eq!(registry.get("blog").unwrap(), "2.0.0");
    }

    #[test]
    fn test_delete_schema_removes_file() {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();

        let resolver = SchemaResolver::new(&schemas_dir);
        let schema = serde_json::json!({"type": "object"});

        resolver.create_schema("blog", "1.0.0", &schema).unwrap();
        resolver.create_schema("blog", "2.0.0", &schema).unwrap();

        resolver.delete_schema("blog", "1.0.0").unwrap();

        let file_path = schemas_dir.join("blog/1.0.0.json");
        assert!(!file_path.exists());
        // Other version should still exist
        assert!(schemas_dir.join("blog/2.0.0.json").exists());
    }

    #[test]
    fn test_delete_schema_not_found() {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();

        let resolver = SchemaResolver::new(&schemas_dir);
        let err = resolver.delete_schema("ghost", "9.9.9").unwrap_err();

        assert!(matches!(err, AppError::SchemaError(_)));
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_delete_latest_updates_registry_to_next() {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();

        let resolver = SchemaResolver::new(&schemas_dir);
        let schema = serde_json::json!({"type": "object"});

        resolver.create_schema("blog", "1.0.0", &schema).unwrap();
        resolver.create_schema("blog", "2.0.0", &schema).unwrap();

        // Delete latest (2.0.0) — registry should fall back to 1.0.0
        resolver.delete_schema("blog", "2.0.0").unwrap();

        let registry = resolver.load_registry().unwrap();
        assert_eq!(registry.get("blog").unwrap(), "1.0.0");
    }

    #[test]
    fn test_delete_non_latest_leaves_registry_unchanged() {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();

        let resolver = SchemaResolver::new(&schemas_dir);
        let schema = serde_json::json!({"type": "object"});

        resolver.create_schema("blog", "1.0.0", &schema).unwrap();
        resolver.create_schema("blog", "2.0.0", &schema).unwrap();

        // Delete non-latest (1.0.0) — registry should still point to 2.0.0
        resolver.delete_schema("blog", "1.0.0").unwrap();

        let registry = resolver.load_registry().unwrap();
        assert_eq!(registry.get("blog").unwrap(), "2.0.0");
    }

    #[test]
    fn test_delete_only_version_removes_registry_entry() {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();

        let resolver = SchemaResolver::new(&schemas_dir);
        let schema = serde_json::json!({"type": "object"});

        resolver.create_schema("blog", "1.0.0", &schema).unwrap();
        resolver.delete_schema("blog", "1.0.0").unwrap();

        let registry = resolver.load_registry().unwrap();
        assert!(!registry.contains_key("blog"));
        // Directory should be cleaned up
        assert!(!schemas_dir.join("blog").exists());
    }
}
