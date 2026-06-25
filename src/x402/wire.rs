//! x402 v2 wire types and PAYMENT-SIGNATURE / PAYMENT-RESPONSE helpers.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

fn default_extensions_object() -> Value {
    serde_json::json!({})
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceInfo {
    pub url: String,
    pub description: String,
    pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentRequired {
    pub x402_version: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub resource: ResourceInfo,
    pub accepts: Vec<Value>,
    #[serde(default = "default_extensions_object")]
    pub extensions: Value,
}

pub fn payment_required_json(pr: &PaymentRequired) -> Result<Value, serde_json::Error> {
    serde_json::to_value(pr)
}

#[derive(Debug, thiserror::Error)]
pub enum PaymentParseError {
    #[error("payment header must be UTF-8")]
    Encoding,
    #[error("payment header must be JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("payment header is not JSON or valid base64 JSON: {0}")]
    Base64(String),
}

pub fn parse_payment_header(raw: &str) -> Result<Value, PaymentParseError> {
    let trimmed = raw.trim();
    if let Ok(v) = serde_json::from_str(trimmed) {
        return Ok(v);
    }
    let bytes = B64
        .decode(trimmed)
        .map_err(|e| PaymentParseError::Base64(e.to_string()))?;
    let s = String::from_utf8(bytes).map_err(|_| PaymentParseError::Encoding)?;
    Ok(serde_json::from_str(&s)?)
}

pub fn extract_payment_header_value(get_header: impl Fn(&str) -> Option<String>) -> Option<String> {
    get_header("payment-signature")
}

pub fn encode_payment_response(settle_result: &Value) -> String {
    B64.encode(settle_result.to_string())
}
