use super::*;

pub(super) fn parse_ttl_policy(value: &Value) -> Result<Option<u64>, RuntimeError> {
    if value.is_null() {
        return Ok(None);
    }
    if let Some(ms) = value.as_u64() {
        return Ok(Some(ms));
    }
    let raw = value.as_str().ok_or_else(|| {
        policy_parse_error("retention", value, "TTL string or millisecond number")
    })?;
    if raw == "forever" || raw == "none" {
        return Ok(None);
    }
    let ttl = raw
        .strip_prefix("ttl_")
        .ok_or_else(|| policy_parse_error("retention", value, "`ttl_<number><unit>`"))?;
    parse_ttl_string(ttl).map(Some)
}

fn parse_ttl_string(raw: &str) -> Result<u64, RuntimeError> {
    let split = raw.find(|ch: char| !ch.is_ascii_digit()).ok_or_else(|| {
        RuntimeError::Other(format!("invalid store retention policy `ttl_{raw}`"))
    })?;
    let (amount, unit) = raw.split_at(split);
    let amount = amount
        .parse::<u64>()
        .map_err(|_| RuntimeError::Other(format!("invalid store retention amount `{amount}`")))?;
    let unit_ms = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => {
            return Err(RuntimeError::Other(format!(
                "invalid store retention unit `{unit}`"
            )))
        }
    };
    Ok(amount.saturating_mul(unit_ms))
}

pub(super) fn parse_u64_policy(name: &str, value: &Value) -> Result<u64, RuntimeError> {
    value
        .as_u64()
        .ok_or_else(|| policy_parse_error(name, value, "unsigned integer"))
}

pub(super) fn parse_bool_policy(name: &str, value: &Value) -> Result<bool, RuntimeError> {
    value
        .as_bool()
        .ok_or_else(|| policy_parse_error(name, value, "boolean"))
}

pub(super) fn parse_string_policy(name: &str, value: &Value) -> Result<String, RuntimeError> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| policy_parse_error(name, value, "string"))
}

fn policy_parse_error(name: &str, value: &Value, expected: &str) -> RuntimeError {
    RuntimeError::Other(format!(
        "invalid store policy `{name}` value {value}: expected {expected}"
    ))
}
