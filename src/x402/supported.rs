use serde_json::Value;

pub fn exact_kind_extra_from_supported(supported: &Value, network: &str) -> Option<Value> {
    let kinds = supported.get("kinds")?.as_array()?;
    let kind = kinds.iter().find(|k| {
        k.get("scheme").and_then(|v| v.as_str()) == Some("exact")
            && k.get("network").and_then(|v| v.as_str()) == Some(network)
    })?;
    kind.get("extra").cloned()
}

pub fn escrow_kind_extra_from_supported(supported: &Value, network: &str) -> Option<Value> {
    let kinds = supported.get("kinds")?.as_array()?;
    let kind = kinds.iter().find(|k| {
        k.get("scheme").and_then(|v| v.as_str()) == Some("sla-escrow")
            && k.get("network").and_then(|v| v.as_str()) == Some(network)
    })?;
    kind.get("extra").cloned()
}
