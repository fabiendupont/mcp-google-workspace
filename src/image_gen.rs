use google_workspace::error::GwsError;
use serde_json::{Value, json};

#[derive(Debug)]
pub struct GeneratedImage {
    pub base64_data: String,
    pub mime_type: String,
}

fn api_err(msg: impl Into<String>) -> GwsError {
    GwsError::Api {
        code: 0,
        message: msg.into(),
        reason: String::new(),
        enable_url: None,
    }
}

fn validate_model(model: &str) -> Result<(), GwsError> {
    if model.contains('/') || model.contains("..") || model.contains('?') || model.contains('&') {
        return Err(GwsError::Validation(format!(
            "Invalid model name '{model}'. Model must not contain '/', '..', '?', or '&'."
        )));
    }
    Ok(())
}

pub fn build_gemini_url(model: &str) -> String {
    format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
        model
    )
}

pub fn build_gemini_request_body(
    prompt: &str,
    aspect_ratio: Option<&str>,
    image_size: Option<&str>,
) -> Value {
    let mut image_config = serde_json::Map::new();
    if let Some(ar) = aspect_ratio {
        image_config.insert("aspectRatio".to_string(), json!(ar));
    }
    if let Some(sz) = image_size {
        image_config.insert("imageSize".to_string(), json!(sz));
    }

    let mut gen_config = json!({
        "responseModalities": ["IMAGE"]
    });
    if !image_config.is_empty() {
        gen_config["imageConfig"] = Value::Object(image_config);
    }

    json!({
        "contents": [{
            "parts": [{ "text": prompt }]
        }],
        "generationConfig": gen_config
    })
}

pub fn parse_gemini_response(response: &Value) -> Result<GeneratedImage, GwsError> {
    let candidates = response
        .get("candidates")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            if let Some(err) = response.get("error") {
                let msg = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("Unknown Gemini API error");
                api_err(msg.to_string())
            } else {
                api_err("No candidates in Gemini response".to_string())
            }
        })?;

    let first = candidates
        .first()
        .ok_or_else(|| api_err("Empty candidates array".to_string()))?;

    let parts = first
        .get("content")
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array())
        .ok_or_else(|| api_err("No parts in candidate content".to_string()))?;

    for part in parts {
        if let Some(inline) = part.get("inlineData").or_else(|| part.get("inline_data")) {
            let data = inline
                .get("data")
                .and_then(|d| d.as_str())
                .ok_or_else(|| api_err("Missing image data in response".to_string()))?;
            let mime = inline
                .get("mimeType")
                .or_else(|| inline.get("mime_type"))
                .and_then(|m| m.as_str())
                .unwrap_or("image/png");
            return Ok(GeneratedImage {
                base64_data: data.to_string(),
                mime_type: mime.to_string(),
            });
        }
    }

    Err(api_err(
        "No image data found in Gemini response parts".to_string(),
    ))
}

/// OAuth scope the Gemini image API needs when no `GEMINI_API_KEY` is set.
const GENERATIVE_LANGUAGE_SCOPE: &str = "https://www.googleapis.com/auth/generative-language";

/// Parameters for the OAuth token used against the Gemini API.
///
/// Kept as a value so the delegation identity (`_meta.user_email`) that is
/// threaded into [`crate::auth::get_token_as`] is explicit and testable
/// without performing network I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TokenRequest<'a> {
    scopes: &'static [&'static str],
    subject: Option<&'a str>,
}

fn token_request(subject: Option<&str>) -> TokenRequest<'_> {
    TokenRequest {
        scopes: &[GENERATIVE_LANGUAGE_SCOPE],
        subject,
    }
}

/// Generate an image as the server principal.
///
/// Wrapper around [`generate_image_as`] kept for existing call sites; new
/// callers should pass the request's delegation subject so multi-user mode
/// honours `_meta.user_email` instead of silently acting as the server
/// identity.
pub async fn generate_image(
    prompt: &str,
    model: &str,
    aspect_ratio: Option<&str>,
    image_size: Option<&str>,
    credentials_file: Option<&str>,
    token_cache: &mut Option<crate::auth::TokenCache>,
) -> Result<GeneratedImage, GwsError> {
    generate_image_as(
        prompt,
        model,
        aspect_ratio,
        image_size,
        credentials_file,
        None,
        token_cache,
    )
    .await
}

/// Generate an image, optionally as a delegated user.
///
/// `subject` must be resolved by the caller through the policy allowlist
/// (`Policy::resolve_delegation_subject(meta.user_email)`), never taken
/// verbatim from client metadata. It is only used when `GEMINI_API_KEY` is
/// unset: an API key authenticates the server principal, so a delegated
/// identity cannot be honoured on that path.
pub async fn generate_image_as(
    prompt: &str,
    model: &str,
    aspect_ratio: Option<&str>,
    image_size: Option<&str>,
    credentials_file: Option<&str>,
    subject: Option<&str>,
    token_cache: &mut Option<crate::auth::TokenCache>,
) -> Result<GeneratedImage, GwsError> {
    validate_model(model)?;
    let api_key = std::env::var("GEMINI_API_KEY").ok();
    let url = build_gemini_url(model);
    let body = build_gemini_request_body(prompt, aspect_ratio, image_size);

    let client = reqwest::Client::new();
    let mut req = client.post(&url).json(&body);

    if let Some(ref key) = api_key {
        req = req.header("x-goog-api-key", key);
    } else {
        let request = token_request(subject);
        let token = crate::auth::get_token_as(
            request.scopes,
            credentials_file,
            Some(token_cache),
            request.subject,
        )
        .await
        .map_err(|_| {
            GwsError::Validation(
                "Gemini auth failed. Set GEMINI_API_KEY env var or configure OAuth \
                 credentials with the generative-language scope."
                    .to_string(),
            )
        })?;
        req = req.bearer_auth(&token);
    }

    let resp = req.send().await.map_err(|_| {
        api_err("Gemini API request failed. Check network connectivity.".to_string())
    })?;

    let status = resp.status();
    let resp_body: Value = resp
        .json()
        .await
        .map_err(|_| api_err("Failed to parse Gemini API response".to_string()))?;

    if !status.is_success() {
        let msg = resp_body
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown error");
        return Err(api_err(format!("Gemini API returned {status}: {msg}")));
    }

    parse_gemini_response(&resp_body)
}

pub fn image_gen_tool_schema() -> Value {
    json!({
        "name": "gws_generate_image",
        "title": "Generate and Insert Image",
        "description": "Generate an image with Gemini and insert it into a Google Doc or Slides presentation.",
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": false,
            "openWorldHint": true
        },
        "inputSchema": {
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Text description of the image to generate"
                },
                "document_id": {
                    "type": "string",
                    "description": "Google Doc to insert the image into"
                },
                "presentation_id": {
                    "type": "string",
                    "description": "Google Slides presentation to insert the image into"
                },
                "position": {
                    "type": "string",
                    "enum": ["start", "end"],
                    "default": "end"
                },
                "width_pt": { "type": "number", "description": "Image width in points" },
                "height_pt": { "type": "number", "description": "Image height in points" },
                "aspect_ratio": {
                    "type": "string",
                    "enum": ["1:1", "3:4", "4:3", "9:16", "16:9"]
                }
            },
            "required": ["prompt"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_url() {
        let url = build_gemini_url("gemini-2.5-flash-image");
        assert!(!url.contains("key="));
        assert!(url.contains("v1beta"));
        assert!(url.contains("gemini-2.5-flash-image:generateContent"));
    }

    #[test]
    fn test_validate_model_rejects_path_traversal() {
        assert!(validate_model("gemini-2.5-flash-image").is_ok());
        assert!(validate_model("../../evil").is_err());
        assert!(validate_model("model?param=1").is_err());
        assert!(validate_model("a/b").is_err());
    }

    #[test]
    fn test_build_request_body_defaults() {
        let body = build_gemini_request_body("a cat", None, None);
        assert_eq!(body["contents"][0]["parts"][0]["text"], "a cat");
        assert_eq!(body["generationConfig"]["responseModalities"][0], "IMAGE");
        assert!(body["generationConfig"].get("imageConfig").is_none());
    }

    #[test]
    fn test_build_request_body_with_options() {
        let body = build_gemini_request_body("a dog", Some("16:9"), Some("2K"));
        assert_eq!(
            body["generationConfig"]["imageConfig"]["aspectRatio"],
            "16:9"
        );
        assert_eq!(body["generationConfig"]["imageConfig"]["imageSize"], "2K");
    }

    #[test]
    fn test_parse_response_success() {
        let resp = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "inlineData": {
                            "data": "iVBORw0KGgo=",
                            "mimeType": "image/png"
                        }
                    }]
                }
            }]
        });
        let img = parse_gemini_response(&resp).unwrap();
        assert_eq!(img.base64_data, "iVBORw0KGgo=");
        assert_eq!(img.mime_type, "image/png");
    }

    #[test]
    fn test_parse_response_snake_case() {
        let resp = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "inline_data": {
                            "data": "abc123",
                            "mime_type": "image/jpeg"
                        }
                    }]
                }
            }]
        });
        let img = parse_gemini_response(&resp).unwrap();
        assert_eq!(img.base64_data, "abc123");
        assert_eq!(img.mime_type, "image/jpeg");
    }

    #[test]
    fn test_parse_response_error() {
        let resp = json!({
            "error": {
                "message": "Rate limit exceeded",
                "code": 429
            }
        });
        let err = parse_gemini_response(&resp).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Rate limit exceeded"));
    }

    #[test]
    fn test_parse_response_no_image() {
        let resp = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "text": "I cannot generate that image"
                    }]
                }
            }]
        });
        let err = parse_gemini_response(&resp).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("No image data"));
    }

    #[test]
    fn test_tool_schema_shape() {
        let schema = image_gen_tool_schema();
        assert_eq!(schema["name"], "gws_generate_image");
        let required = schema["inputSchema"]["required"].as_array().unwrap();
        assert!(required.contains(&json!("prompt")));
        let props = schema["inputSchema"]["properties"].as_object().unwrap();
        assert!(props.contains_key("prompt"));
        assert!(props.contains_key("document_id"));
        assert!(props.contains_key("presentation_id"));
        assert!(props.contains_key("aspect_ratio"));
    }

    #[test]
    fn test_token_request_carries_delegated_subject() {
        let request = token_request(Some("alice@example.com"));
        assert_eq!(request.subject, Some("alice@example.com"));
        assert_eq!(request.scopes, &[GENERATIVE_LANGUAGE_SCOPE]);
    }

    #[test]
    fn test_token_request_without_subject() {
        let request = token_request(None);
        assert_eq!(request.subject, None);
    }

    /// Compile/arity guard: `generate_image` must keep its original signature
    /// (a concurrent `server.rs` edit still calls it) and delegate with no
    /// subject. The futures are never polled, so no auth or network I/O runs.
    #[test]
    fn test_generate_image_wrapper_needs_no_subject() {
        let mut cache: Option<crate::auth::TokenCache> = None;
        drop(generate_image(
            "a cat",
            "gemini-2.5-flash-image",
            None,
            None,
            None,
            &mut cache,
        ));

        let mut cache: Option<crate::auth::TokenCache> = None;
        drop(generate_image_as(
            "a cat",
            "gemini-2.5-flash-image",
            Some("16:9"),
            Some("2K"),
            None,
            Some("alice@example.com"),
            &mut cache,
        ));
    }
}
