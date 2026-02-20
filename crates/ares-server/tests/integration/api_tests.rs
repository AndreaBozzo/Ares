use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use crate::integration::common::{TEST_API_KEY, setup_test_app};

#[tokio::test]
async fn health_returns_200() {
    let (app, _container) = setup_test_app().await;

    let response = app
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
    let (app, _container) = setup_test_app().await;

    let response = app
        .oneshot(Request::get("/v1/jobs").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn wrong_api_key_returns_401() {
    let (app, _container) = setup_test_app().await;

    let response = app
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
async fn create_and_get_job() {
    let (app, _container) = setup_test_app().await;

    let create_body = serde_json::json!({
        "url": "https://example.com",
        "schema_name": "test",
        "schema": {"type": "object"},
        "model": "gpt-4o-mini",
        "base_url": "https://api.openai.com/v1"
    });

    // Create job
    let response = app
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
