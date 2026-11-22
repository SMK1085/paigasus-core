// SPDX-License-Identifier: Apache-2.0

//! The S3 client of the memory adapter (spec §6.8 and §6.9).
//!
//! `s3_builder` is a pure function of the config and an injected environment. `build_store`
//! builds the client from it. The client never reads the process environment on its own.

use std::sync::Arc;
use std::time::Duration;

use object_store::aws::{AmazonS3Builder, S3ConditionalPut};
use object_store::{Certificate, ClientOptions, ObjectStore, RetryConfig};

use crate::config::MemoryS3Config;

/// The only environment variables that reach the client. All of them name a credential source.
/// A variable that changes the endpoint, the signing or the trust roots is not in the list.
pub const ENV_ALLOWLIST: [&str; 10] = [
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_WEB_IDENTITY_TOKEN_FILE",
    "AWS_ROLE_ARN",
    "AWS_ROLE_SESSION_NAME",
    "AWS_ENDPOINT_URL_STS",
    "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
    "AWS_CONTAINER_CREDENTIALS_FULL_URI",
    "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
];

/// The time limit of one request.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// The time limit to connect.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The most retries of one request.
pub const MAX_RETRIES: usize = 3;
/// The time limit of one request with all its retries.
pub const RETRY_TIMEOUT: Duration = Duration::from_secs(10);

/// The client could not be built. No variant holds a credential or an endpoint.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The CA bundle file cannot be read.
    #[error("the CA bundle file cannot be read: {path}")]
    CaRead { path: String },
    /// The CA bundle file holds no certificate.
    #[error("the CA bundle file holds no certificate: {path}")]
    CaEmpty { path: String },
    /// The CA bundle file is not valid PEM.
    #[error("the CA bundle file is not valid PEM: {path}")]
    CaParse { path: String },
    /// The store client cannot be built.
    #[error("the S3 client cannot be built (class: {class})")]
    Build { class: String },
}

/// Make the builder from the config and the environment.
///
/// Only the names in [`ENV_ALLOWLIST`] are copied, and only when the value is not empty. The
/// config values are set after the environment, so a config value always wins.
///
/// # Errors
///
/// A [`ClientError`] when the CA bundle cannot be read or holds no certificate.
pub fn s3_builder(cfg: &MemoryS3Config, env: impl IntoIterator<Item = (String, String)>) -> Result<AmazonS3Builder, ClientError> {
    let mut builder = AmazonS3Builder::new();
    for (name, value) in env {
        if value.is_empty() || !ENV_ALLOWLIST.contains(&name.as_str()) {
            continue;
        }
        let key = name.to_ascii_lowercase().parse().map_err(|_| ClientError::Build { class: "env_key".into() })?;
        builder = builder.with_config(key, value);
    }
    builder = builder
        .with_bucket_name(&cfg.bucket)
        .with_region(&cfg.region)
        .with_virtual_hosted_style_request(!cfg.path_style())
        .with_allow_http(cfg.allow_http)
        .with_conditional_put(S3ConditionalPut::ETagMatch)
        .with_disable_bulk_delete(true)
        .with_retry(RetryConfig {
            max_retries: MAX_RETRIES,
            retry_timeout: RETRY_TIMEOUT,
            ..Default::default()
        });
    if let Some(endpoint) = &cfg.endpoint {
        builder = builder.with_endpoint(endpoint);
    }
    // One call only: `with_client_options` replaces every client option.
    let mut options = ClientOptions::new().with_timeout(REQUEST_TIMEOUT).with_connect_timeout(CONNECT_TIMEOUT).with_allow_http(cfg.allow_http);
    if let Some(path) = &cfg.extra_ca_bundle_path {
        let pem = std::fs::read(path).map_err(|_| ClientError::CaRead { path: path.clone() })?;
        let certs = Certificate::from_pem_bundle(&pem).map_err(|_| ClientError::CaParse { path: path.clone() })?;
        if certs.is_empty() {
            return Err(ClientError::CaEmpty { path: path.clone() });
        }
        // These ADD to the platform roots (spec §6.9).
        for cert in certs {
            options = options.with_root_certificate(cert);
        }
    }
    Ok(builder.with_client_options(options))
}

/// Build the store client.
///
/// The function installs the `ring` crypto provider first, because reqwest 0.13 has no default.
/// The production caller passes `std::env::vars_os()` filtered to UTF-8 pairs.
///
/// Proxy (measured, spec G5, Review Focus 6): reqwest 0.13.5 reads `HTTP_PROXY`, `HTTPS_PROXY`,
/// `ALL_PROXY` and `NO_PROXY` when no `proxy` and no `no_proxy` is set. See
/// `reqwest-0.13.5/src/async_impl/client.rs` line 417 (`ProxyMatcher::system()` under
/// `auto_sys_proxy`, which is `true` by default, line 310). `object_store` sets neither, so these
/// variables can change the client. The README names them. The client does not turn them off.
///
/// # Errors
///
/// A [`ClientError`] from [`s3_builder`], or `Build` when the client cannot be made.
pub fn build_store(cfg: &MemoryS3Config, env: impl IntoIterator<Item = (String, String)>) -> Result<Arc<dyn ObjectStore>, ClientError> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let store = s3_builder(cfg, env)?.build().map_err(|_| ClientError::Build { class: "build".into() })?;
    Ok(Arc::new(store))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MemoryS3Config;
    use object_store::ClientConfigKey;
    use object_store::aws::{AmazonS3ConfigKey, S3ConditionalPut};

    fn cfg(endpoint: Option<&str>, ca: Option<&str>) -> MemoryS3Config {
        MemoryS3Config {
            bucket: "bucket".into(),
            prefix: "p".into(),
            region: "us-east-1".into(),
            endpoint: endpoint.map(str::to_string),
            path_style: None,
            allow_http: false,
            extra_ca_bundle_path: ca.map(str::to_string),
            apply_deadline_secs: 10,
        }
    }

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn every_allowlisted_name_is_a_config_key() {
        for name in ENV_ALLOWLIST {
            assert!(name.to_ascii_lowercase().parse::<AmazonS3ConfigKey>().is_ok(), "{name}");
        }
    }

    #[test]
    fn the_environment_cannot_change_the_client() {
        let e = env(&[
            ("AWS_ENDPOINT_URL_S3", "http://evil"),
            ("AWS_ENDPOINT", "http://evil"),
            ("AWS_ENDPOINT_URL", "http://evil"),
            ("AWS_ALLOW_INVALID_CERTIFICATES", "true"),
            ("AWS_SKIP_SIGNATURE", "true"),
            ("AWS_DISABLE_SYSTEM_CERTIFICATES", "true"),
            ("AWS_CONDITIONAL_PUT", "disabled"),
            ("AWS_REGION", "eu-west-1"),
        ]);
        let b = s3_builder(&cfg(Some("https://s3.example"), None), e).unwrap();
        assert_eq!(b.get_config_value(&AmazonS3ConfigKey::Endpoint).as_deref(), Some("https://s3.example"));
        assert_eq!(b.get_config_value(&AmazonS3ConfigKey::S3Endpoint), None);
        assert_eq!(b.get_config_value(&AmazonS3ConfigKey::SkipSignature).as_deref(), Some("false"));
        assert_eq!(b.get_config_value(&AmazonS3ConfigKey::ConditionalPut), Some(S3ConditionalPut::ETagMatch.to_string()));
        assert_eq!(b.get_config_value(&AmazonS3ConfigKey::Region).as_deref(), Some("us-east-1"));
        assert_eq!(b.get_config_value(&AmazonS3ConfigKey::DisableBulkDelete).as_deref(), Some("true"));
        assert_eq!(b.get_config_value(&AmazonS3ConfigKey::Client(ClientConfigKey::AllowInvalidCertificates)).as_deref(), Some("false"));
    }

    #[test]
    fn allowlisted_credentials_reach_the_builder() {
        let e = env(&[("AWS_ACCESS_KEY_ID", "AKIDTEST"), ("AWS_SECRET_ACCESS_KEY", "secret"), ("AWS_SESSION_TOKEN", "tok")]);
        let b = s3_builder(&cfg(None, None), e).unwrap();
        assert_eq!(b.get_config_value(&AmazonS3ConfigKey::AccessKeyId).as_deref(), Some("AKIDTEST"));
        assert_eq!(b.get_config_value(&AmazonS3ConfigKey::SecretAccessKey).as_deref(), Some("secret"));
        assert_eq!(b.get_config_value(&AmazonS3ConfigKey::Token).as_deref(), Some("tok"));
    }

    #[test]
    fn an_empty_allowlisted_variable_is_not_copied() {
        let b = s3_builder(&cfg(None, None), env(&[("AWS_ACCESS_KEY_ID", "")])).unwrap();
        assert_eq!(b.get_config_value(&AmazonS3ConfigKey::AccessKeyId), None);
    }

    #[test]
    fn an_empty_ca_bundle_is_an_error() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), "no pem block here\n").unwrap();
        let p = f.path().to_str().unwrap();
        assert!(matches!(s3_builder(&cfg(None, Some(p)), env(&[])), Err(ClientError::CaEmpty { .. })));
    }

    #[test]
    fn a_missing_ca_bundle_is_an_error() {
        let r = s3_builder(&cfg(None, Some("/nonexistent/ca.pem")), env(&[]));
        assert!(matches!(r, Err(ClientError::CaRead { .. })));
    }

    #[test]
    fn a_ca_bundle_adds_roots() {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), cert.cert.pem()).unwrap();
        let p = f.path().to_str().unwrap();
        let c = cfg(None, Some(p));
        assert!(s3_builder(&c, env(&[])).is_ok());
        assert!(build_store(&c, env(&[])).is_ok());
    }

    #[test]
    fn no_client_error_text_holds_a_credential() {
        let secret = env(&[("AWS_SECRET_ACCESS_KEY", "secret-value")]);
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), "x").unwrap();
        let p = f.path().to_str().unwrap().to_string();
        let errs = [
            s3_builder(&cfg(None, Some(&p)), secret.clone()).err().unwrap(),
            s3_builder(&cfg(None, Some("/nonexistent/ca.pem")), secret.clone()).err().unwrap(),
            ClientError::CaParse { path: p.clone() },
            ClientError::Build { class: "other".into() },
        ];
        for e in errs {
            assert!(!e.to_string().contains("secret-value"), "{e}");
        }
    }
}
