use serde_json::Value;

#[derive(Debug, Clone)]
pub struct EscrowFundDetails {
    pub payment_uid: String,
    pub oracle_authority: String,
    pub payer_wallet: String,
    pub tx_signature: String,
}

pub fn extract_escrow_fund_details(fund: &Value, proof: &Value) -> Option<EscrowFundDetails> {
    let payment_uid = fund
        .get("paymentUid")
        .or_else(|| fund.get("payment_uid"))
        .and_then(|v| v.as_str())
        .map(normalize_payment_uid_hex)
        .filter(|s| s.len() == 64)?;
    let oracle_authority = fund
        .get("oracleAuthority")
        .or_else(|| fund.get("oracle_authority"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_string();
    let payer = fund
        .get("payer")
        .and_then(|v| v.as_str())
        .or_else(|| {
            proof
                .get("paymentPayload")
                .and_then(|p| p.get("payer"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("anonymous")
        .to_string();
    let tx_sig = fund
        .get("transaction")
        .and_then(|v| v.as_str())
        .or_else(|| {
            proof
                .get("paymentPayload")
                .and_then(|p| p.pointer("/payload/transaction"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("")
        .to_string();
    Some(EscrowFundDetails {
        payment_uid,
        oracle_authority,
        payer_wallet: payer,
        tx_signature: tx_sig,
    })
}

fn normalize_payment_uid_hex(raw: &str) -> String {
    raw.trim().trim_start_matches("0x").to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_escrow_fund_details_reads_camel_case_fields() {
        let fund = json!({
            "paymentUid": "ab".repeat(32),
            "oracleAuthority": "Oracle111111111111111111111111111111111111",
            "payer": "Buyer111111111111111111111111111111111111",
            "transaction": "tx-sig"
        });
        let details = extract_escrow_fund_details(&fund, &json!({})).expect("details");
        assert_eq!(details.payment_uid, "ab".repeat(32));
        assert_eq!(
            details.oracle_authority,
            "Oracle111111111111111111111111111111111111"
        );
        assert_eq!(details.payer_wallet, "Buyer111111111111111111111111111111111111");
        assert_eq!(details.tx_signature, "tx-sig");
    }
}
