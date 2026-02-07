use std::path::Path;

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
}
