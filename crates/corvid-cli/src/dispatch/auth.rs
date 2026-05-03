use super::*;

pub(crate) fn cmd_auth(command: AuthCommand) -> Result<u8> {
    match command {
        AuthCommand::Migrate {
            auth_state,
            approvals_state,
        } => {
            let out = auth_cmd::run_auth_migrate(auth_cmd::AuthMigrateArgs {
                auth_state,
                approvals_state,
            })?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "auth_state": out.auth_state,
                    "approvals_state": out.approvals_state,
                    "auth_initialised": out.auth_initialised,
                    "approvals_initialised": out.approvals_initialised,
                }))?
            );
            Ok(0)
        }
        AuthCommand::Keys { command } => match command {
            AuthKeysCommand::Issue {
                auth_state,
                key_id,
                service_actor,
                tenant,
                raw_key,
                scope_fingerprint,
                display_name,
                expires_at_ms,
            } => {
                let out = auth_cmd::run_auth_key_issue(auth_cmd::AuthKeyIssueArgs {
                    auth_state,
                    key_id,
                    service_actor_id: service_actor,
                    tenant_id: tenant,
                    raw_key,
                    scope_fingerprint,
                    display_name,
                    expires_at_ms: expires_at_ms.unwrap_or(u64::MAX),
                })?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "key_id": out.key_id,
                        "service_actor_id": out.service_actor_id,
                        "tenant_id": out.tenant_id,
                        "key_hash_prefix": out.key_hash_prefix,
                        "scope_fingerprint": out.scope_fingerprint,
                        "expires_at_ms": out.expires_at_ms,
                        "raw_key": out.raw_key,
                    }))?
                );
                Ok(0)
            }
            AuthKeysCommand::Revoke { auth_state, key_id } => {
                let out = auth_cmd::run_auth_key_revoke(auth_cmd::AuthKeyRevokeArgs {
                    auth_state,
                    key_id,
                })?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "key_id": out.key_id,
                        "revoked_at_ms": out.revoked_at_ms,
                    }))?
                );
                Ok(0)
            }
            AuthKeysCommand::Rotate {
                auth_state,
                key_id,
                service_actor,
                tenant,
                new_key_id,
                new_raw_key,
                expires_at_ms,
            } => {
                let out = auth_cmd::run_auth_key_rotate(auth_cmd::AuthKeyRotateArgs {
                    auth_state,
                    key_id,
                    service_actor_id: service_actor,
                    tenant_id: tenant,
                    new_key_id,
                    new_raw_key,
                    expires_at_ms: expires_at_ms.unwrap_or(u64::MAX),
                })?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "revoked_key_id": out.revoked_key_id,
                        "new_key_id": out.new_key_id,
                        "raw_key": out.raw_key,
                        "scope_fingerprint": out.scope_fingerprint,
                        "expires_at_ms": out.expires_at_ms,
                    }))?
                );
                Ok(0)
            }
        },
    }
}
