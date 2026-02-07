use ares_core::models::NewExtraction;
use ares_db::ExtractionRepository;

use crate::integration::common::setup_test_db;

#[tokio::test]
async fn save_and_retrieve_extraction() {
    let (pool, _container) = setup_test_db().await;
    let repo = ExtractionRepository::new(pool);

    let extraction = NewExtraction {
        url: "https://example.com".into(),
        schema_name: "blog".into(),
        extracted_data: serde_json::json!({"title": "Hello World"}),
        raw_content_hash: "abc123".repeat(10),
        data_hash: "def456".repeat(10),
        model: "gpt-4o-mini".into(),
    };

    let id = repo.save(&extraction).await.unwrap();
    assert!(!id.is_nil());

    let latest = repo
        .get_latest("https://example.com", "blog")
        .await
        .unwrap()
        .expect("Should find the extraction");

    assert_eq!(latest.id, id);
    assert_eq!(latest.url, "https://example.com");
    assert_eq!(latest.schema_name, "blog");
    assert_eq!(
        latest.extracted_data,
        serde_json::json!({"title": "Hello World"})
    );
    assert_eq!(latest.model, "gpt-4o-mini");
}

#[tokio::test]
async fn get_latest_returns_most_recent() {
    let (pool, _container) = setup_test_db().await;
    let repo = ExtractionRepository::new(pool);

    // Insert two extractions for the same URL+schema
    let e1 = NewExtraction {
        url: "https://example.com".into(),
        schema_name: "blog".into(),
        extracted_data: serde_json::json!({"title": "First"}),
        raw_content_hash: "hash1".into(),
        data_hash: "dhash1".into(),
        model: "model-a".into(),
    };
    let _id1 = repo.save(&e1).await.unwrap();

    // Small delay to ensure different timestamps
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let e2 = NewExtraction {
        url: "https://example.com".into(),
        schema_name: "blog".into(),
        extracted_data: serde_json::json!({"title": "Second"}),
        raw_content_hash: "hash2".into(),
        data_hash: "dhash2".into(),
        model: "model-b".into(),
    };
    let id2 = repo.save(&e2).await.unwrap();

    let latest = repo
        .get_latest("https://example.com", "blog")
        .await
        .unwrap()
        .expect("Should find the extraction");

    assert_eq!(latest.id, id2);
    assert_eq!(
        latest.extracted_data,
        serde_json::json!({"title": "Second"})
    );
}

#[tokio::test]
async fn get_latest_returns_none_for_unknown() {
    let (pool, _container) = setup_test_db().await;
    let repo = ExtractionRepository::new(pool);

    let result = repo
        .get_latest("https://nonexistent.com", "blog")
        .await
        .unwrap();

    assert!(result.is_none());
}

#[tokio::test]
async fn get_history_returns_ordered_with_limit() {
    let (pool, _container) = setup_test_db().await;
    let repo = ExtractionRepository::new(pool);

    for i in 0..5 {
        let e = NewExtraction {
            url: "https://example.com".into(),
            schema_name: "blog".into(),
            extracted_data: serde_json::json!({"index": i}),
            raw_content_hash: format!("chash{i}"),
            data_hash: format!("dhash{i}"),
            model: "model".into(),
        };
        repo.save(&e).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let history = repo
        .get_history("https://example.com", "blog", 3)
        .await
        .unwrap();

    assert_eq!(history.len(), 3);
    // Should be newest first
    assert_eq!(history[0].extracted_data["index"], 4);
    assert_eq!(history[1].extracted_data["index"], 3);
    assert_eq!(history[2].extracted_data["index"], 2);
}

#[tokio::test]
async fn health_check_succeeds() {
    let (pool, _container) = setup_test_db().await;
    let repo = ExtractionRepository::new(pool);

    repo.health_check().await.unwrap();
}
