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
