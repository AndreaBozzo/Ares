use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use crate::integration::common::{TEST_API_KEY, setup_test_app, setup_test_app_no_auth};

#[tokio::test]
async fn health_returns_200() {
    let app = setup_test_app().await;

    let response = app
        .router
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "healthy");
    assert_eq!(json["database"], "ok");
}

#[tokio::test]
async fn unauthenticated_request_returns_401() {
    let app = setup_test_app().await;

    let response = app
        .router
        .oneshot(Request::get("/v1/jobs").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn wrong_api_key_returns_401() {
    let app = setup_test_app().await;

    let response = app
        .router
        .oneshot(
            Request::get("/v1/jobs")
                .header("authorization", "Bearer wrong-key")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn no_admin_token_returns_403() {
    let app = setup_test_app_no_auth().await;

    let response = app
        .router
        .oneshot(
            Request::get("/v1/jobs")
                .header("authorization", "Bearer any-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "forbidden");
}

#[tokio::test]
async fn create_and_get_job() {
    let app = setup_test_app().await;

    let create_body = serde_json::json!({
        "url": "https://example.com",
        "schema_name": "test",
        "schema": {"type": "object"},
        "model": "gpt-4o-mini",
        "base_url": "https://api.openai.com/v1"
    });

    // Create job
    let response = app
        .router
        .clone()
        .oneshot(
            Request::post("/v1/jobs")
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "pending");
    let job_id = json["job_id"].as_str().unwrap();

    // Get job
    let response = app
        .router
        .oneshot(
            Request::get(format!("/v1/jobs/{job_id}"))
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["id"], job_id);
    assert_eq!(json["status"], "pending");
    assert_eq!(json["url"], "https://example.com");
}

// ---------------------------------------------------------------------------
// Schema endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_schemas_empty() {
    let app = setup_test_app().await;

    // Write an empty registry
    std::fs::write(app.schemas_dir.join("registry.json"), "{}").unwrap();

    let response = app
        .router
        .oneshot(
            Request::get("/v1/schemas")
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["schemas"], serde_json::json!([]));
}

#[tokio::test]
async fn create_and_list_schema() {
    let app = setup_test_app().await;

    let create_body = serde_json::json!({
        "name": "blog",
        "version": "1.0.0",
        "schema": {"type": "object", "properties": {"title": {"type": "string"}}}
    });

    // Create schema
    let response = app
        .router
        .clone()
        .oneshot(
            Request::post("/v1/schemas")
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["name"], "blog");
    assert_eq!(json["version"], "1.0.0");

    // List schemas
    let response = app
        .router
        .clone()
        .oneshot(
            Request::get("/v1/schemas")
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["schemas"][0]["name"], "blog");
    assert_eq!(json["schemas"][0]["latest_version"], "1.0.0");
    assert_eq!(json["schemas"][0]["versions"], serde_json::json!(["1.0.0"]));
}

#[tokio::test]
async fn get_schema_returns_content() {
    let app = setup_test_app().await;

    let schema = serde_json::json!({"type": "object", "properties": {"url": {"type": "string"}}});

    // Create schema first
    let create_body = serde_json::json!({
        "name": "links",
        "version": "2.0.0",
        "schema": schema,
    });

    let response = app
        .router
        .clone()
        .oneshot(
            Request::post("/v1/schemas")
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    // Get schema
    let response = app
        .router
        .oneshot(
            Request::get("/v1/schemas/links/2.0.0")
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["name"], "links");
    assert_eq!(json["version"], "2.0.0");
    assert_eq!(json["schema"], schema);
}

#[tokio::test]
async fn get_schema_not_found() {
    let app = setup_test_app().await;

    let response = app
        .router
        .oneshot(
            Request::get("/v1/schemas/missing/1.0.0")
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "not_found");
}

// ---------------------------------------------------------------------------
// Cancel job endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancel_pending_job() {
    let app = setup_test_app().await;

    // Create a job first
    let create_body = serde_json::json!({
        "url": "https://example.com",
        "schema_name": "test",
        "schema": {"type": "object"},
        "model": "gpt-4o-mini",
        "base_url": "https://api.openai.com/v1"
    });

    let response = app
        .router
        .clone()
        .oneshot(
            Request::post("/v1/jobs")
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "pending");
    let job_id = json["job_id"].as_str().unwrap();

    // Cancel it
    let response = app
        .router
        .oneshot(
            Request::delete(format!("/v1/jobs/{job_id}"))
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn cancel_nonexistent_job() {
    let app = setup_test_app().await;

    let fake_id = uuid::Uuid::new_v4();
    let response = app
        .router
        .oneshot(
            Request::delete(format!("/v1/jobs/{fake_id}"))
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// List jobs endpoint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_jobs_with_status_filter() {
    let app = setup_test_app().await;

    let create_body = serde_json::json!({
        "url": "https://example.com/1",
        "schema_name": "test",
        "schema": {"type": "object"},
        "model": "gpt-4o-mini",
        "base_url": "https://api.openai.com/v1"
    });

    // Create two jobs
    for _ in 0..2 {
        let response = app
            .router
            .clone()
            .oneshot(
                Request::post("/v1/jobs")
                    .header("authorization", format!("Bearer {TEST_API_KEY}"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&create_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    // List with status=pending
    let response = app
        .router
        .oneshot(
            Request::get("/v1/jobs?status=pending")
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["total"], 2);
    assert_eq!(json["jobs"].as_array().unwrap().len(), 2);
}

// ---------------------------------------------------------------------------
// Invalid request body
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_job_invalid_body() {
    let app = setup_test_app().await;

    let response = app
        .router
        .oneshot(
            Request::post("/v1/jobs")
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"invalid": true}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    // Axum returns 422 for deserialization failures
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

// ---------------------------------------------------------------------------
// Extractions endpoint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_extractions_empty() {
    let app = setup_test_app().await;

    let response = app
        .router
        .oneshot(
            Request::get("/v1/extractions?url=https://example.com&schema_name=test")
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["total"], 0);
    assert_eq!(json["extractions"].as_array().unwrap().len(), 0);
}

// ---------------------------------------------------------------------------
// Retry job endpoint
// ---------------------------------------------------------------------------

/// Helper: create a job and return its ID.
async fn create_test_job(app: &crate::integration::common::TestApp) -> String {
    let create_body = serde_json::json!({
        "url": "https://example.com",
        "schema_name": "test",
        "schema": {"type": "object"},
        "model": "gpt-4o-mini",
        "base_url": "https://api.openai.com/v1"
    });

    let response = app
        .router
        .clone()
        .oneshot(
            Request::post("/v1/jobs")
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    json["job_id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn retry_cancelled_job() {
    let app = setup_test_app().await;
    let job_id = create_test_job(&app).await;

    // Cancel the job
    let response = app
        .router
        .clone()
        .oneshot(
            Request::delete(format!("/v1/jobs/{job_id}"))
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Retry it
    let response = app
        .router
        .clone()
        .oneshot(
            Request::post(format!("/v1/jobs/{job_id}/retry"))
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["id"], job_id);
    assert_eq!(json["status"], "pending");
    assert_eq!(json["retry_count"], 0);
    assert!(json["error_message"].is_null());
}

#[tokio::test]
async fn retry_pending_job_returns_409() {
    let app = setup_test_app().await;
    let job_id = create_test_job(&app).await;

    // Try to retry a pending job — should fail
    let response = app
        .router
        .oneshot(
            Request::post(format!("/v1/jobs/{job_id}/retry"))
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "conflict");
}

#[tokio::test]
async fn retry_nonexistent_job_returns_404() {
    let app = setup_test_app().await;

    let fake_id = uuid::Uuid::new_v4();
    let response = app
        .router
        .oneshot(
            Request::post(format!("/v1/jobs/{fake_id}/retry"))
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_jobs_with_pagination() {
    let app = setup_test_app().await;

    // Create 3 jobs
    for _ in 0..3 {
        create_test_job(&app).await;
    }

    // First page: limit=2, offset=0
    let response = app
        .router
        .clone()
        .oneshot(
            Request::get("/v1/jobs?limit=2&offset=0")
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["total"], 3);
    assert_eq!(json["limit"], 2);
    assert_eq!(json["offset"], 0);
    assert_eq!(json["jobs"].as_array().unwrap().len(), 2);

    // Second page: limit=2, offset=2
    let response = app
        .router
        .clone()
        .oneshot(
            Request::get("/v1/jobs?limit=2&offset=2")
                .header("authorization", format!("Bearer {TEST_API_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["total"], 3);
    assert_eq!(json["limit"], 2);
    assert_eq!(json["offset"], 2);
    assert_eq!(json["jobs"].as_array().unwrap().len(), 1);
}
