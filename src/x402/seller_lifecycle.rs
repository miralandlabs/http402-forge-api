use serde_json::Value;

/// pr402 `/sellers/{wallet}/preview`: PDAs are always derived at preview time
/// (`lifecycle.previewed`). Only `lifecycle.activated == true` means the SplitVault
/// account exists on-chain. Do **not** treat `schemes.exact.vaultPda` as activation.
pub fn vault_activated_from_preview(preview: &Value) -> bool {
    preview
        .pointer("/lifecycle/activated")
        .and_then(|v| v.as_bool())
        == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn derived_pda_without_activation_is_not_active() {
        let preview = json!({
            "lifecycle": { "previewed": true, "activated": false },
            "schemes": {
                "exact": {
                    "vaultPda": "9AKmHTQNd1jakQ9XhNtoCMXtc8RCBfATVqNyA8qy64yd",
                    "status": "NotProvisioned"
                }
            }
        });
        assert!(!vault_activated_from_preview(&preview));
    }

    #[test]
    fn lifecycle_activated_means_active() {
        let preview = json!({
            "lifecycle": { "activated": true },
            "schemes": { "exact": { "vaultPda": "Vault111", "status": "Active" } }
        });
        assert!(vault_activated_from_preview(&preview));
    }
}
