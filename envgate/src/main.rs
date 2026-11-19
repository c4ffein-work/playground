mod acme;
mod certs;
mod config;
mod proxy;

use std::process::ExitCode;
use std::sync::Arc;

use log::{error, info};
use pingora::listeners::tls::TlsSettings;
use pingora::proxy::http_proxy_service;
use pingora::server::Server;
use pingora::server::configuration::ServerConf;
use pingora::services::background::background_service;

use crate::acme::{AcmeService, ChallengeStore};
use crate::certs::{BoxError, CertStore};
use crate::config::{Config, TlsMode};

fn main() -> ExitCode {
    let vars = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)));
    let config = match Config::from_env(vars) {
        Ok(config) => config,
        Err(problems) => {
            eprintln!("envgate: invalid configuration");
            for problem in problems {
                eprintln!("  - {problem}");
            }
            return ExitCode::from(2);
        }
    };

    env_logger::Builder::new()
        .filter_level(config.log_level)
        .format_target(false)
        .init();

    match run(config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("startup failed: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(config: Config) -> Result<(), BoxError> {
    let store = Arc::new(CertStore::new());
    let challenges = Arc::new(ChallengeStore::default());

    for route in &config.routes {
        let key = match (&route.cert_files, config.tls) {
            (Some(files), _) => Some(Arc::new(certs::certified_key_from_files(
                &files.cert,
                &files.key,
            )?)),
            (None, TlsMode::SelfSigned) => None,
            _ => continue,
        };
        for host in &route.hosts {
            let key = match &key {
                Some(key) => key.clone(),
                None => Arc::new(certs::self_signed(host)?),
            };
            store.set(host, key);
        }
    }

    let acme_hosts = config.acme_hosts();
    let acme = if acme_hosts.is_empty() {
        None
    } else {
        Some(AcmeService::new(
            config.acme.clone(),
            acme_hosts,
            &config.data_dir,
            store.clone(),
            challenges.clone(),
        )?)
    };

    let conf = ServerConf {
        threads: config.threads,
        grace_period_seconds: Some(config.shutdown_grace_seconds),
        graceful_shutdown_timeout_seconds: Some(3),
        ..Default::default()
    };
    let mut server = Server::new_with_opt_and_conf(None, conf);
    server.bootstrap();

    let mut service = http_proxy_service(
        &server.configuration,
        proxy::Gateway::new(&config, challenges),
    );
    service.add_tcp(&config.http_addr.to_string());
    info!("listening for HTTP on {}", config.http_addr);

    if config.tls != TlsMode::Off {
        // The paths are unused: every certificate comes from the resolver.
        let mut tls = TlsSettings::intermediate("", "")?;
        tls.set_cert_resolver(store);
        tls.enable_h2();
        service.add_tls_with_settings(&config.https_addr.to_string(), None, tls);
        info!(
            "listening for HTTPS on {} ({:?})",
            config.https_addr, config.tls
        );
    }
    for route in &config.routes {
        let upstreams: Vec<String> = route.upstreams.iter().map(|u| u.authority()).collect();
        info!(
            "route {}: {}{} -> {}",
            route.name,
            route.hosts.join(","),
            route.path,
            upstreams.join(",")
        );
    }

    server.add_service(service);
    if let Some(acme) = acme {
        server.add_service(background_service("acme", acme));
    }
    server.run_forever()
}
