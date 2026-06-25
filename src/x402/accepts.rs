use serde_json::{json, Value};

use crate::config::{ClusterConfig, DEFAULT_SCHEME_ESCROW, DEFAULT_SCHEME_EXACT};

pub fn listing_uses_escrow(delivery_scheme: &str, byte_size: i64, threshold: u64) -> bool {
    delivery_scheme == "escrow" || byte_size as u64 >= threshold
}

pub fn build_accepts_for_listing(
    cluster: &ClusterConfig,
    pay_to: &str,
    amount_micro_usdc: i64,
    timeout_secs: u64,
    use_escrow: bool,
    platform_fee_bps: u16,
    platform_fee_wallet: Option<&str>,
) -> Vec<Value> {
    let amount_str = amount_micro_usdc.to_string();
    let scheme = if use_escrow {
        DEFAULT_SCHEME_ESCROW
    } else {
        DEFAULT_SCHEME_EXACT
    };

    let mut line = json!({
        "scheme": scheme,
        "network": cluster.network,
        "asset": cluster.usdc_mint,
        "amount": amount_str,
        "payTo": pay_to,
        "maxTimeoutSeconds": timeout_secs,
    });

    if platform_fee_bps > 0 {
        if let Some(wallet) = platform_fee_wallet {
            if let Some(obj) = line.as_object_mut() {
                obj.insert(
                    "split".into(),
                    json!({
                        "platformWallet": wallet,
                        "platformBps": platform_fee_bps
                    }),
                );
            }
        }
    }

    vec![line]
}

pub fn idempotency_key(payment_sig: &str, canonical_path: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(payment_sig.as_bytes());
    hasher.update(canonical_path.as_bytes());
    format!("{:x}", hasher.finalize())
}
