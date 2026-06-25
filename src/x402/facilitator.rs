use reqwest::Client;
use serde_json::Value;
use thiserror::Error;

use crate::config::AppConfig;
use crate::x402::facilitator_client::{FacilitatorClient, FacilitatorError};
use crate::x402::supported::{escrow_kind_extra_from_supported, exact_kind_extra_from_supported};

#[derive(Debug, Error)]
pub enum FacilitatorExtError {
    #[error("facilitator: {0}")]
    Inner(#[from] FacilitatorError),
    #[error("http: {0}")]
    Http(String),
}

#[derive(Clone)]
pub struct Facilitator {
    client: FacilitatorClient,
    http: Client,
    base: String,
}

impl Facilitator {
    pub fn new(config: &AppConfig) -> Result<Self, FacilitatorExtError> {
        let base = config
            .facilitator_base_url
            .trim_end_matches('/')
            .to_string();
        Ok(Self {
            client: FacilitatorClient::new(&base)?,
            http: Client::builder()
                .timeout(std::time::Duration::from_secs(
                    config.facilitator_timeout_secs,
                ))
                .build()
                .map_err(|e| FacilitatorExtError::Http(e.to_string()))?,
            base,
        })
    }

    pub async fn verify_and_settle(&self, body: &Value) -> Result<Value, FacilitatorExtError> {
        self.client
            .verify_and_settle(body)
            .await
            .map_err(Into::into)
    }

    pub async fn seller_has_vault(&self, wallet: &str) -> Result<bool, FacilitatorExtError> {
        self.client
            .seller_has_vault(&self.base, wallet)
            .await
            .map_err(Into::into)
    }

    pub async fn fetch_seller_preview(
        &self,
        wallet: &str,
    ) -> Result<serde_json::Value, FacilitatorExtError> {
        let url = format!("{}/api/v1/facilitator/sellers/{wallet}/preview", self.base);
        let res = self
            .http
            .get(&url)
            .header("X-API-Version", "1")
            .send()
            .await
            .map_err(|e| FacilitatorExtError::Http(e.to_string()))?;
        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(FacilitatorExtError::Http(format!(
                "seller preview {status}: {body}"
            )));
        }
        res.json()
            .await
            .map_err(|e| FacilitatorExtError::Http(e.to_string()))
    }

    pub async fn build_provision_tx(
        &self,
        wallet: &str,
        asset: &str,
    ) -> Result<serde_json::Value, FacilitatorExtError> {
        let url = format!("{}/api/v1/facilitator/sellers/provision-tx", self.base);
        let res = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .header("X-API-Version", "1")
            .json(&serde_json::json!({ "wallet": wallet, "asset": asset }))
            .send()
            .await
            .map_err(|e| FacilitatorExtError::Http(e.to_string()))?;
        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(FacilitatorExtError::Http(format!(
                "provision-tx {status}: {body}"
            )));
        }
        res.json()
            .await
            .map_err(|e| FacilitatorExtError::Http(e.to_string()))
    }

    pub fn seller_dashboard_url(&self) -> String {
        self.base
            .trim_end_matches('/')
            .strip_suffix("/api/v1/facilitator")
            .unwrap_or(self.base.trim_end_matches('/'))
            .to_string()
    }

    pub async fn resolve_vault_pda(
        &self,
        merchant_wallet: &str,
    ) -> Result<String, FacilitatorExtError> {
        let url = format!(
            "{}/api/v1/facilitator/sellers/{merchant_wallet}/rails/exact",
            self.base
        );
        let res = self
            .http
            .get(&url)
            .header("X-API-Version", "1")
            .send()
            .await
            .map_err(|e| FacilitatorExtError::Http(e.to_string()))?;
        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(FacilitatorExtError::Http(format!(
                "rails/exact {status}: {body}"
            )));
        }
        let json: Value = res
            .json()
            .await
            .map_err(|e| FacilitatorExtError::Http(e.to_string()))?;
        json.get("vaultPda")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| FacilitatorExtError::Http("rails/exact missing vaultPda".into()))
    }

    async fn fetch_supported(&self) -> Result<Value, FacilitatorExtError> {
        let supported_url = format!("{}/api/v1/facilitator/supported", self.base);
        self.http
            .get(&supported_url)
            .header("X-API-Version", "1")
            .send()
            .await
            .map_err(|e| FacilitatorExtError::Http(e.to_string()))?
            .json()
            .await
            .map_err(|e| FacilitatorExtError::Http(e.to_string()))
    }

    pub async fn enrich_accepts(
        &self,
        mut lines: Vec<Value>,
        merchant_wallet: &str,
        network: &str,
        use_escrow: bool,
        oracle_authorities: &[String],
        oracle_profile_id: &str,
    ) -> Result<Vec<Value>, FacilitatorExtError> {
        let supported = self.fetch_supported().await?;
        let extra = if use_escrow {
            escrow_kind_extra_from_supported(&supported, network)
                .unwrap_or_else(|| serde_json::json!({}))
        } else {
            exact_kind_extra_from_supported(&supported, network)
                .unwrap_or_else(|| serde_json::json!({}))
        };

        let vault = if use_escrow {
            None
        } else {
            self.resolve_vault_pda(merchant_wallet).await.ok()
        };

        for line in &mut lines {
            if let Some(obj) = line.as_object_mut() {
                if let Some(vault_pda) = &vault {
                    obj.insert("payTo".into(), Value::String(vault_pda.clone()));
                }
                let mut line_extra = extra.clone();
                if let Some(extra_obj) = line_extra.as_object_mut() {
                    extra_obj.insert(
                        "merchantWallet".into(),
                        Value::String(merchant_wallet.to_string()),
                    );
                    if use_escrow && !oracle_authorities.is_empty() {
                        extra_obj.insert(
                            "oracleAuthorities".into(),
                            Value::Array(
                                oracle_authorities
                                    .iter()
                                    .cloned()
                                    .map(Value::String)
                                    .collect(),
                            ),
                        );
                        extra_obj.insert(
                            "oracleProfiles".into(),
                            Value::Array(vec![serde_json::json!({
                                "profileId": oracle_profile_id,
                                "operatorPubkey": oracle_authorities[0],
                                "normativeSpecUrl": "https://github.com/miraland-labs/oracles/blob/main/docs/SELLER_GUIDE.md"
                            })]),
                        );
                    }
                }
                obj.insert("extra".into(), line_extra);
            }
        }
        Ok(lines)
    }
}
