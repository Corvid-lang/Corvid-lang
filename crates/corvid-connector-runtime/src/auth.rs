use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorAuthState {
    pub tenant_id: String,
    pub actor_id: String,
    pub token_id: String,
    pub scopes: BTreeSet<String>,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectorAuthError {
    MissingTenant,
    MissingActor,
    MissingToken,
    ExpiredToken,
    RevokedRefreshToken,
    TenantMismatch,
    MissingScope(String),
}

impl std::fmt::Display for ConnectorAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingTenant => write!(f, "connector auth requires tenant_id"),
            Self::MissingActor => write!(f, "connector auth requires actor_id"),
            Self::MissingToken => write!(f, "connector auth requires token_id"),
            Self::ExpiredToken => write!(f, "connector auth token is expired"),
            Self::RevokedRefreshToken => write!(f, "connector refresh token is revoked"),
            Self::TenantMismatch => write!(f, "connector refresh token tenant mismatch"),
            Self::MissingScope(scope) => write!(f, "connector token missing scope `{scope}`"),
        }
    }
}

impl std::error::Error for ConnectorAuthError {}

impl ConnectorAuthState {
    pub fn new(
        tenant_id: impl Into<String>,
        actor_id: impl Into<String>,
        token_id: impl Into<String>,
        scopes: impl IntoIterator<Item = impl Into<String>>,
        expires_at_ms: u64,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            actor_id: actor_id.into(),
            token_id: token_id.into(),
            scopes: scopes.into_iter().map(Into::into).collect(),
            expires_at_ms,
        }
    }

    pub fn authorize(&self, required_scope: &str, now_ms: u64) -> Result<(), ConnectorAuthError> {
        if self.tenant_id.trim().is_empty() {
            return Err(ConnectorAuthError::MissingTenant);
        }
        if self.actor_id.trim().is_empty() {
            return Err(ConnectorAuthError::MissingActor);
        }
        if self.token_id.trim().is_empty() {
            return Err(ConnectorAuthError::MissingToken);
        }
        if self.expires_at_ms <= now_ms {
            return Err(ConnectorAuthError::ExpiredToken);
        }
        if !self.scopes.contains(required_scope) {
            return Err(ConnectorAuthError::MissingScope(required_scope.to_string()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorRefreshTokenState {
    pub tenant_id: String,
    pub actor_id: String,
    pub refresh_token_id: String,
    pub scopes: BTreeSet<String>,
    pub revoked: bool,
}

impl ConnectorRefreshTokenState {
    pub fn new(
        tenant_id: impl Into<String>,
        actor_id: impl Into<String>,
        refresh_token_id: impl Into<String>,
        scopes: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            actor_id: actor_id.into(),
            refresh_token_id: refresh_token_id.into(),
            scopes: scopes.into_iter().map(Into::into).collect(),
            revoked: false,
        }
    }

    pub fn refresh(
        &self,
        tenant_id: &str,
        new_token_id: impl Into<String>,
        expires_at_ms: u64,
    ) -> Result<ConnectorAuthState, ConnectorAuthError> {
        if self.revoked {
            return Err(ConnectorAuthError::RevokedRefreshToken);
        }
        if self.tenant_id != tenant_id {
            return Err(ConnectorAuthError::TenantMismatch);
        }
        Ok(ConnectorAuthState {
            tenant_id: self.tenant_id.clone(),
            actor_id: self.actor_id.clone(),
            token_id: new_token_id.into(),
            scopes: self.scopes.clone(),
            expires_at_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_token_mints_tenant_scoped_access_state() {
        let refresh = ConnectorRefreshTokenState::new(
            "tenant-1",
            "actor-1",
            "refresh-1",
            ["ms365.mail_search", "ms365.calendar_events"],
        );
        let access = refresh.refresh("tenant-1", "access-2", 1000).unwrap();
        assert_eq!(access.tenant_id, "tenant-1");
        assert_eq!(access.actor_id, "actor-1");
        assert!(access.scopes.contains("ms365.mail_search"));
        access.authorize("ms365.calendar_events", 1).unwrap();
    }

    #[test]
    fn refresh_rejects_cross_tenant_or_revoked_token() {
        let refresh = ConnectorRefreshTokenState::new("tenant-1", "actor-1", "refresh-1", ["a"]);
        let err = refresh.refresh("tenant-2", "access", 100).unwrap_err();
        assert_eq!(err, ConnectorAuthError::TenantMismatch);

        let mut revoked = refresh.clone();
        revoked.revoked = true;
        let err = revoked.refresh("tenant-1", "access", 100).unwrap_err();
        assert_eq!(err, ConnectorAuthError::RevokedRefreshToken);
    }
}
