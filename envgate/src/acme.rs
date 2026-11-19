//! Certificate issuance and renewal over ACME (HTTP-01), run as a background
//! service inside the proxy.
//!
//! Each host gets its own certificate, so one host with broken DNS cannot
//! hold up the others. Certificates and the account key live under
//! `DATA_DIR`; that directory has to survive restarts, or every restart
//! requests new certificates and runs into the CA's rate limits.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier, NewAccount,
    NewOrder, OrderStatus, RetryPolicy,
};
use log::{error, info, warn};
use pingora::server::ShutdownWatch;
use pingora::services::background::BackgroundService;

use crate::certs::{self, BoxError, CertStore, Validity};
use crate::config::AcmeConfig;

pub const CHALLENGE_PREFIX: &str = "/.well-known/acme-challenge/";

const CHECK_INTERVAL: Duration = Duration::from_secs(60);
const ISSUE_TIMEOUT: Duration = Duration::from_secs(300);
/// Let's Encrypt allows five failed validations per host per hour, so the
/// first retry is already slower than that and it only backs off from there.
const RETRY_BASE: Duration = Duration::from_secs(15 * 60);
const RETRY_MAX: Duration = Duration::from_secs(12 * 60 * 60);

/// Pending HTTP-01 challenges: token -> key authorization. Shared with the
/// proxy, which answers the CA's validation requests on the HTTP listener.
#[derive(Default)]
pub struct ChallengeStore {
    pending: Mutex<HashMap<String, String>>,
}

impl ChallengeStore {
    pub fn get(&self, token: &str) -> Option<String> {
        self.pending.lock().unwrap().get(token).cloned()
    }

    fn insert(&self, token: String, key_authorization: String) {
        self.pending
            .lock()
            .unwrap()
            .insert(token, key_authorization);
    }

    fn remove(&self, token: &str) {
        self.pending.lock().unwrap().remove(token);
    }
}

pub struct AcmeService {
    config: AcmeConfig,
    hosts: Vec<String>,
    /// `DATA_DIR/acme/<ca host>`: keeps staging and production material apart,
    /// so switching CA never reuses a certificate from the other one.
    dir: PathBuf,
    store: Arc<CertStore>,
    challenges: Arc<ChallengeStore>,
    /// Validity of the certificates found on disk at startup.
    initial: HashMap<String, Validity>,
}

impl AcmeService {
    /// Loads what is already on disk into `store`. Hosts without a usable
    /// certificate get a self-signed placeholder until the first issuance
    /// succeeds, so the listener answers (with a browser warning) rather than
    /// refusing the handshake.
    pub fn new(
        config: AcmeConfig,
        hosts: Vec<String>,
        data_dir: &Path,
        store: Arc<CertStore>,
        challenges: Arc<ChallengeStore>,
    ) -> Result<Self, BoxError> {
        let ca = config
            .directory_url
            .trim_start_matches("https://")
            .split('/')
            .next()
            .unwrap_or("ca")
            .replace(':', "_");
        let dir = data_dir.join("acme").join(ca);
        std::fs::create_dir_all(dir.join("certs")).map_err(|e| {
            format!(
                "cannot create {}: {e} (is DATA_DIR writable?)",
                dir.display()
            )
        })?;

        let mut initial = HashMap::new();
        for host in &hosts {
            let (cert, key) = cert_paths(&dir, host);
            let loaded = cert
                .exists()
                .then(|| certs::certified_key_from_files(&cert, &key))
                .transpose()
                .and_then(|k| k.map(|k| Validity::of(&k).map(|v| (k, v))).transpose());
            match loaded {
                Ok(Some((key, validity))) if validity.not_after > certs::unix_now() => {
                    info!("{host}: loaded certificate from {}", cert.display());
                    store.set(host, Arc::new(key));
                    initial.insert(host.clone(), validity);
                    continue;
                }
                Ok(Some(_)) => warn!("{host}: stored certificate has expired"),
                Ok(None) => {}
                Err(e) => warn!("{host}: ignoring stored certificate: {e}"),
            }
            store.set(host, Arc::new(certs::self_signed(host)?));
        }

        Ok(Self {
            config,
            hosts,
            dir,
            store,
            challenges,
            initial,
        })
    }

    async fn account(&self) -> Result<Account, BoxError> {
        let builder = || match &self.config.ca_file {
            Some(path) => Account::builder_with_root(path),
            None => Account::builder(),
        };
        let path = self.dir.join("account.json");
        if path.exists() {
            let credentials: AccountCredentials = serde_json::from_slice(&std::fs::read(&path)?)?;
            return Ok(builder()?.from_credentials(credentials).await?);
        }

        let contact: Vec<String> = self
            .config
            .email
            .iter()
            .map(|e| format!("mailto:{e}"))
            .collect();
        let contact: Vec<&str> = contact.iter().map(String::as_str).collect();
        let (account, credentials) = builder()?
            .create(
                &NewAccount {
                    contact: &contact,
                    terms_of_service_agreed: true,
                    only_return_existing: false,
                },
                self.config.directory_url.clone(),
                None,
            )
            .await?;
        certs::write_private(&path, &serde_json::to_vec_pretty(&credentials)?)?;
        info!("created ACME account at {}", self.config.directory_url);
        Ok(account)
    }

    async fn issue(&self, account: &Account, host: &str) -> Result<Validity, BoxError> {
        let identifiers = [Identifier::Dns(host.to_string())];
        let mut order = account.new_order(&NewOrder::new(&identifiers)).await?;

        let mut tokens = Vec::new();
        let outcome = async {
            let mut authorizations = order.authorizations();
            while let Some(result) = authorizations.next().await {
                let mut authz = result?;
                match authz.status {
                    AuthorizationStatus::Pending => {}
                    AuthorizationStatus::Valid => continue,
                    other => return Err(format!("authorization is {other:?}").into()),
                }
                let mut challenge = authz
                    .challenge(ChallengeType::Http01)
                    .ok_or("the CA offered no HTTP-01 challenge")?;
                let token = challenge.token.clone();
                self.challenges.insert(
                    token.clone(),
                    challenge.key_authorization().as_str().to_string(),
                );
                tokens.push(token);
                challenge.set_ready().await?;
            }

            let status = order.poll_ready(&RetryPolicy::default()).await?;
            if status != OrderStatus::Ready {
                return Err(format!(
                    "validation failed (order is {status:?}): the CA must reach \
                     http://{host}{CHALLENGE_PREFIX} on port 80"
                )
                .into());
            }
            let key_pem = order.finalize().await?;
            let chain_pem = order.poll_certificate(&RetryPolicy::default()).await?;
            Ok::<_, BoxError>((chain_pem, key_pem))
        }
        .await;
        for token in &tokens {
            self.challenges.remove(token);
        }
        let (chain_pem, key_pem) = outcome?;

        let key = certs::certified_key_from_pem(chain_pem.as_bytes(), key_pem.as_bytes())?;
        let validity = Validity::of(&key)?;

        // Key first: a crash between the two writes then leaves a pair that
        // fails the "keys match" check on load instead of an unusable mix.
        let (cert_path, key_path) = cert_paths(&self.dir, host);
        std::fs::create_dir_all(cert_path.parent().expect("has a parent"))?;
        certs::write_private(&key_path, key_pem.as_bytes())?;
        certs::write_private(&cert_path, chain_pem.as_bytes())?;

        self.store.set(host, Arc::new(key));
        Ok(validity)
    }

    async fn run_once(
        &self,
        state: &mut HashMap<String, HostState>,
        account: &mut Option<Account>,
    ) {
        let now = certs::unix_now();
        for host in &self.hosts {
            let entry = state.entry(host.clone()).or_default();
            let due = entry.validity.is_none_or(|v| now >= v.renew_at());
            if !due || now < entry.retry_after {
                continue;
            }

            if account.is_none() {
                match self.account().await {
                    Ok(a) => *account = Some(a),
                    Err(e) => {
                        error!("ACME account setup failed: {e}");
                        entry.fail(now);
                        // Without an account nothing else can succeed either.
                        return;
                    }
                }
            }
            let acct = account.as_ref().expect("set above");

            info!("{host}: requesting certificate");
            let result = tokio::time::timeout(ISSUE_TIMEOUT, self.issue(acct, host)).await;
            match result.unwrap_or_else(|_| Err("timed out".into())) {
                Ok(validity) => {
                    info!(
                        "{host}: certificate issued, valid for {} days",
                        (validity.not_after - now) / 86_400
                    );
                    *entry = HostState {
                        validity: Some(validity),
                        ..Default::default()
                    };
                }
                Err(e) => {
                    let wait = entry.fail(now);
                    error!(
                        "{host}: certificate request failed, retrying in {} min: {e}",
                        wait / 60
                    );
                }
            }
        }
    }
}

#[derive(Default)]
struct HostState {
    validity: Option<Validity>,
    failures: u32,
    retry_after: i64,
}

impl HostState {
    /// Records a failure and returns how many seconds to wait before retrying.
    fn fail(&mut self, now: i64) -> i64 {
        let wait = RETRY_BASE
            .saturating_mul(1u32 << self.failures.min(10))
            .min(RETRY_MAX)
            .as_secs() as i64;
        self.failures += 1;
        self.retry_after = now + wait;
        wait
    }
}

fn cert_paths(dir: &Path, host: &str) -> (PathBuf, PathBuf) {
    let base = dir.join("certs").join(host);
    (base.join("fullchain.pem"), base.join("privkey.pem"))
}

#[async_trait]
impl BackgroundService for AcmeService {
    async fn start(&self, mut shutdown: ShutdownWatch) {
        let mut state: HashMap<String, HostState> = self
            .initial
            .iter()
            .map(|(host, v)| {
                let state = HostState {
                    validity: Some(*v),
                    ..Default::default()
                };
                (host.clone(), state)
            })
            .collect();
        let mut account = None;

        loop {
            tokio::select! {
                _ = self.run_once(&mut state, &mut account) => {}
                _ = shutdown.changed() => return,
            }
            tokio::select! {
                _ = tokio::time::sleep(CHECK_INTERVAL) => {}
                _ = shutdown.changed() => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_backs_off_and_is_capped() {
        let mut state = HostState::default();
        assert_eq!(state.fail(0), 15 * 60);
        assert_eq!(state.fail(0), 30 * 60);
        assert_eq!(state.fail(0), 60 * 60);
        for _ in 0..20 {
            state.fail(0);
        }
        assert_eq!(state.fail(1000), 12 * 60 * 60);
        assert_eq!(state.retry_after, 1000 + 12 * 60 * 60);
    }

    #[test]
    fn challenge_store_round_trip() {
        let store = ChallengeStore::default();
        assert_eq!(store.get("t"), None);
        store.insert("t".into(), "t.thumb".into());
        assert_eq!(store.get("t").as_deref(), Some("t.thumb"));
        store.remove("t");
        assert_eq!(store.get("t"), None);
    }
}
