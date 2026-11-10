mod callback;
pub mod setup;
mod store;

use anyhow::{Context, Result, bail};
use reqwest::{Client, StatusCode, Url};
use rusticnotion::NOTION_API_VERSION;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fmt,
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use tokio::{net::TcpListener, sync::Mutex};

pub use store::TokenStore;

const DEFAULT_REDIRECT: &str = "http://127.0.0.1:8765/oauth/callback";
const API_BASE: &str = "https://api.notion.com/v1";

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Notion authorization expired or was rejected")]
    Unauthorized,
    #[error("OAuth grant is invalid or expired; run notion-tui --login")]
    InvalidGrant,
    #[error("Notion authorization was cancelled")]
    Denied,
    #[error("OAuth login timed out after five minutes; run notion-tui --login to try again")]
    Timeout,
}

pub fn is_unauthorized(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<AuthError>(),
            Some(AuthError::Unauthorized)
        ) || matches!(cause.downcast_ref::<rusticnotion::Error>(),
                Some(rusticnotion::Error::ApiError { error }) if error.status.code() == 401)
    })
}

pub fn env_value(name: &str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value.trim().to_owned()).filter(|value| !value.is_empty())),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => bail!("{name} must contain valid Unicode"),
    }
}

#[derive(Clone)]
pub struct OAuthConfig {
    client_id: String,
    client_secret: String,
    redirect: Url,
}

impl OAuthConfig {
    fn from_lookup(lookup: impl Fn(&str) -> Result<Option<String>>) -> Result<Self> {
        let required = |name| {
            lookup(name)?.filter(|value| !value.trim().is_empty()).with_context(||
            format!("Set {name} for OAuth login. Create a Notion OAuth integration and register {DEFAULT_REDIRECT}, or set NOTION_API_TOKEN. See README.md"))
        };
        let config = Self {
            client_id: required("NOTION_OAUTH_CLIENT_ID")?,
            client_secret: required("NOTION_OAUTH_CLIENT_SECRET")?,
            redirect: Url::parse(
                &lookup("NOTION_OAUTH_REDIRECT_URI")?.unwrap_or_else(|| DEFAULT_REDIRECT.into()),
            )
            .map_err(|_| {
                anyhow::anyhow!(
                    "NOTION_OAUTH_REDIRECT_URI must be an HTTP loopback URL with an explicit port"
                )
            })?,
        };
        config.callback_address()?;
        Ok(config)
    }

    fn callback_address(&self) -> Result<SocketAddr> {
        let host = match self.redirect.host_str() {
            Some("localhost") => Some(IpAddr::from([127, 0, 0, 1])),
            Some(host) => host
                .trim_matches(['[', ']'])
                .parse::<IpAddr>()
                .ok()
                .filter(IpAddr::is_loopback),
            None => None,
        };
        if self.redirect.scheme() != "http"
            || host.is_none()
            || self.redirect.port().is_none()
            || !self.redirect.username().is_empty()
            || self.redirect.password().is_some()
            || self.redirect.query().is_some()
            || self.redirect.fragment().is_some()
        {
            bail!(
                "NOTION_OAUTH_REDIRECT_URI must be an HTTP loopback URL with an explicit port and no credentials, query, or fragment"
            );
        }
        let address = SocketAddr::new(host.unwrap(), self.redirect.port().unwrap());
        if address.port() == 0 {
            bail!("The OAuth callback needs a fixed nonzero port registered with Notion");
        }
        Ok(address)
    }
}

// Deliberately omit Debug: credentials must never appear in diagnostic output.
#[derive(Clone, Serialize, Deserialize)]
pub struct Credentials {
    version: u8,
    client_id: String,
    access_token: String,
    refresh_token: String,
    workspace_id: String,
    workspace_name: Option<String>,
    workspace_icon: Option<String>,
    bot_id: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    workspace_id: String,
    workspace_name: Option<String>,
    workspace_icon: Option<String>,
    bot_id: String,
}

#[derive(Clone)]
struct OAuthClient {
    config: OAuthConfig,
    http: Client,
    api_base: String,
}

impl OAuthClient {
    fn new(config: OAuthConfig) -> Result<Self> {
        Ok(Self {
            config,
            api_base: API_BASE.into(),
            http: Client::builder()
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
    }

    fn authorization_url(&self, state: &str) -> Result<Url> {
        let mut url = Url::parse(&format!("{}/oauth/authorize", self.api_base))?;
        url.query_pairs_mut().extend_pairs([
            ("client_id", self.config.client_id.as_str()),
            ("redirect_uri", self.config.redirect.as_str()),
            ("response_type", "code"),
            ("owner", "user"),
            ("state", state),
        ]);
        Ok(url)
    }

    async fn exchange(&self, payload: Value) -> Result<Credentials> {
        let response = self.http.post(format!("{}/oauth/token", self.api_base))
            .basic_auth(&self.config.client_id, Some(&self.config.client_secret))
            .header("Notion-Version", NOTION_API_VERSION).json(&payload).send().await
            .map_err(|_| anyhow::anyhow!("Could not contact Notion's OAuth token endpoint; credentials were not removed. Try again when connected"))?;
        let status = response.status();
        if !status.is_success() {
            let error = response.json::<Value>().await.unwrap_or_default();
            if error["error"] == "invalid_grant" {
                return Err(AuthError::InvalidGrant.into());
            }
            // Provider error descriptions may echo submitted secrets; never display the body.
            bail!(
                "Notion OAuth token exchange returned HTTP {status}; check the OAuth client configuration or try --login again"
            );
        }
        let token: TokenResponse = response
            .json()
            .await
            .map_err(|_| anyhow::anyhow!("Notion returned an invalid OAuth token response"))?;
        if token.access_token.is_empty() || token.refresh_token.is_empty() {
            bail!("Notion returned an incomplete OAuth token pair");
        }
        Ok(Credentials {
            version: 1,
            client_id: self.config.client_id.clone(),
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            workspace_id: token.workspace_id,
            workspace_name: token.workspace_name,
            workspace_icon: token.workspace_icon,
            bot_id: token.bot_id,
        })
    }

    async fn validate(&self, token: &str) -> Result<()> {
        let status = self.http.get(format!("{}/users/me", self.api_base)).bearer_auth(token)
            .header("Notion-Version", NOTION_API_VERSION).send().await
            .map_err(|_| anyhow::anyhow!("Could not validate saved Notion login; check your connection. Saved credentials were preserved"))?.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(AuthError::Unauthorized.into());
        }
        if !status.is_success() {
            bail!(
                "Notion login validation returned HTTP {status}; saved credentials were preserved"
            );
        }
        Ok(())
    }

    async fn authorize(&self) -> Result<Credentials> {
        let address = self.config.callback_address()?;
        let listener = TcpListener::bind(address).await
            .context("Could not bind OAuth callback port; close the other listener or configure another registered loopback redirect URI")?;
        let state = random_state()?;
        let url = self.authorization_url(&state)?;
        println!(
            "Opening Notion login. If the browser does not open, visit:\n{url}\n\nWaiting for authorization. Press Ctrl-C to cancel."
        );
        let browser_url = url.to_string();
        let opened = tokio::task::spawn_blocking(move || open::that_detached(browser_url)).await;
        if !matches!(opened, Ok(Ok(()))) {
            println!("Could not open a browser automatically; use the URL above.");
        }
        let code = callback::wait(
            listener,
            self.config.redirect.path(),
            &state,
            Duration::from_secs(300),
        )
        .await?;
        self.exchange(json!({"grant_type":"authorization_code", "code":code,
            "redirect_uri": self.config.redirect.as_str()}))
            .await
    }
}

fn random_state() -> Result<String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("Could not obtain randomness for OAuth login"))?;
    use fmt::Write;
    let mut state = String::with_capacity(64);
    for byte in bytes {
        write!(state, "{byte:02x}")?;
    }
    Ok(state)
}

pub struct OAuthSession {
    client: OAuthClient,
    store: TokenStore,
    credentials: Mutex<Credentials>,
}

impl OAuthSession {
    pub async fn connect(config: OAuthConfig, store: TokenStore, force: bool) -> Result<Self> {
        let client = OAuthClient::new(config)?;
        Self::connect_with(client, store, force, |client| async move {
            client.authorize().await
        })
        .await
    }

    async fn connect_with<F, Fut>(
        client: OAuthClient,
        store: TokenStore,
        force: bool,
        authorize: F,
    ) -> Result<Self>
    where
        F: FnOnce(OAuthClient) -> Fut,
        Fut: std::future::Future<Output = Result<Credentials>>,
    {
        if !force && let Some(session) = Self::restore(client.clone(), store.clone()).await? {
            return Ok(session);
        }
        let credentials = authorize(client.clone()).await?;
        let _lock = store.lock().await?;
        store.save(&credentials)?;
        Ok(Self {
            client,
            store,
            credentials: Mutex::new(credentials),
        })
    }

    async fn restore(client: OAuthClient, store: TokenStore) -> Result<Option<Self>> {
        let credentials = {
            let _lock = store.lock().await?;
            store
                .load()?
                .filter(|credentials| credentials.client_id == client.config.client_id)
        };
        let Some(credentials) = credentials else {
            return Ok(None);
        };
        let token = credentials.access_token.clone();
        let session = Self {
            client,
            store,
            credentials: Mutex::new(credentials),
        };
        match session.client.validate(&token).await {
            Ok(()) => Ok(Some(session)),
            Err(error) if is_unauthorized(&error) => match session.refresh(&token).await {
                Ok(()) => match session.client.validate(&session.token().await).await {
                    Ok(()) => Ok(Some(session)),
                    Err(error) if is_unauthorized(&error) => Ok(None),
                    Err(error) => Err(error),
                },
                Err(error)
                    if matches!(
                        error.downcast_ref::<AuthError>(),
                        Some(AuthError::InvalidGrant)
                    ) =>
                {
                    Ok(None)
                }
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        }
    }

    pub fn api_base(&self) -> &str {
        &self.client.api_base
    }
    pub async fn token(&self) -> String {
        self.credentials.lock().await.access_token.clone()
    }

    pub async fn refresh(&self, rejected_token: &str) -> Result<()> {
        let mut credentials = self.credentials.lock().await;
        if credentials.access_token != rejected_token {
            return Ok(());
        }
        let _lock = self.store.lock().await?;
        let saved = self
            .store
            .load()?
            .context("Saved OAuth login was removed; run notion-tui --login")?;
        if saved.client_id != self.client.config.client_id {
            bail!("OAuth configuration changed; run notion-tui --login");
        }
        if saved.access_token != rejected_token {
            *credentials = saved;
            return Ok(());
        }
        let updated = self
            .client
            .exchange(json!({"grant_type":"refresh_token", "refresh_token":saved.refresh_token}))
            .await?;
        // Keep the valid rotated pair in memory even if persistence subsequently fails.
        *credentials = updated;
        self.store.save(&credentials).context("Login refreshed but could not be saved; fix config-directory permissions or run --login next time")
    }
}

#[cfg(test)]
mod tests;
