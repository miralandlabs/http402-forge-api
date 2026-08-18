//! sla-escrow fund bind fields from facilitator verify/settle JSON (existing ABI).

use serde_json::Value;

fn json_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn first_str(root: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = json_str(root, key) {
            return Some(s.to_string());
        }
        if let Some(extra) = root.get("extra") {
            if let Some(s) = json_str(extra, key) {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn pointer_str(root: &Value, pointer: &str) -> Option<String> {
    root.pointer(pointer)
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub fn normalize_payment_uid(raw: &str) -> Option<String> {
    let s = raw.trim().trim_start_matches("0x").to_ascii_lowercase();
    if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(s)
    } else {
        None
    }
}

/// `(payment_uid_hex, oracle_authority)` from a successful sla-escrow fund/verify/settle body.
pub fn escrow_fund_bind_from_wire(settle: &Value, proof: &Value) -> Option<(String, String)> {
    let uid_raw = first_str(settle, &["paymentUid", "payment_uid"])
        .or_else(|| first_str(proof, &["paymentUid", "payment_uid"]))
        .or_else(|| pointer_str(proof, "/paymentPayload/payload/paymentUid"))
        .or_else(|| pointer_str(proof, "/paymentPayload/payload/payment_uid"))
        .or_else(|| pointer_str(settle, "/paymentPayload/payload/paymentUid"))?;
    let uid = normalize_payment_uid(&uid_raw)?;

    let oracle = first_str(settle, &["oracleAuthority", "oracle_authority"])
        .or_else(|| first_str(proof, &["oracleAuthority", "oracle_authority"]))
        .or_else(|| pointer_str(proof, "/paymentRequirements/extra/oracleAuthority"))
        .or_else(|| pointer_str(settle, "/extra/oracleAuthority"))?;

    Some((uid, oracle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bind_fields_from_settle_camel_case() {
        let uid = "ab".repeat(32);
        let settle = json!({
            "success": true,
            "payer": "Buyer111",
            "paymentUid": uid,
            "oracleAuthority": "Oracle111"
        });
        let proof = json!({});
        let (got_uid, got_oracle) = escrow_fund_bind_from_wire(&settle, &proof).unwrap();
        assert_eq!(got_uid, uid);
        assert_eq!(got_oracle, "Oracle111");
    }

    #[test]
    fn bind_fields_reject_short_payment_uid() {
        let settle = json!({
            "paymentUid": "abcd",
            "oracleAuthority": "Oracle111"
        });
        assert!(escrow_fund_bind_from_wire(&settle, &json!({})).is_none());
    }
}
