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
    MissingScope(String),
}

impl std::fmt::Display for ConnectorAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingTenant => write!(f, "connector auth requires tenant_id"),
            Self::MissingActor => write!(f, "connector auth requires actor_id"),
            Self::MissingToken => write!(f, "connector auth requires token_id"),
            Self::ExpiredToken => write!(f, "connector auth token is expired"),
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
