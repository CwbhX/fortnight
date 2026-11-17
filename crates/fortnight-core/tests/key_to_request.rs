//! The chain the app runs once the user unlocks: key file → unlock →
//! OpenAI-compatible client → request with `Authorization: Bearer <key>`.

use fortnight_core::llm::{ChatRequest, LlmClient, LlmConfig};
use fortnight_core::openai::OpenAiClient;
use fortnight_core::vault;
use secrecy::SecretString;
use serde_json::json;
use wiremock::matchers::{bearer_token, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn unlocked_key_is_sent_as_bearer_token() {
    let dir = std::env::temp_dir().join(format!("fortnight-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = vault::key_path(&dir);
    vault::save(
        &file,
        &SecretString::from("sk-test-key".to_string()),
        &SecretString::from("correct horse".to_string()),
    )
    .unwrap();
    assert!(vault::unlock(&file, &SecretString::from("wrong pass".to_string())).is_err());
    let key = vault::unlock(&file, &SecretString::from("correct horse".to_string())).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(bearer_token("sk-test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "m",
            "choices": [{"finish_reason": "stop", "message": {"content": "OK"}}]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = OpenAiClient::new(LlmConfig {
        base_url: format!("{}/v1/", server.uri()),
        model: "m".into(),
        api_key: Some(key),
    })
    .unwrap();
    let reply = client.chat(ChatRequest::test()).await.unwrap();
    assert_eq!(reply.text, "OK");
}
