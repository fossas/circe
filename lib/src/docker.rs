use std::{collections::HashMap, process::Stdio};

use crate::{homedir, Authentication, Reference};
use base64::Engine;
use color_eyre::{
    eyre::{eyre, Context, OptionExt, Result},
    Section, SectionExt,
};
use serde::Deserialize;
use tap::TapFallible;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info, warn};

impl Authentication {
    /// Read authentication information for the host from the configured Docker credentials, if any.
    ///
    /// Reference:
    /// - https://docs.docker.com/reference/cli/docker/login
    /// - https://github.com/docker/docker-credential-helpers
    pub async fn docker(target: &Reference) -> Result<Self> {
        match Self::docker_internal(target).await {
            Ok(auth) => {
                debug!("inferred docker auth: {auth:?}");
                Ok(auth)
            }
            Err(err) => {
                warn!(?err, "unable to infer docker auth; trying unauthenticated");
                Ok(Authentication::None)
            }
        }
    }

    async fn docker_internal(target: &Reference) -> Result<Self> {
        let host = &target.host;
        let path = homedir()
            .context("get home directory")?
            .join(".docker")
            .join("config.json");

        let config = tokio::fs::read_to_string(&path)
            .await
            .context("read docker config")
            .with_section(|| path.display().to_string().header("Config file path:"))?;

        serde_json::from_str::<DockerConfig>(&config)
            .context("parse docker config")
            .with_section(|| path.display().to_string().header("Config file path:"))
            .with_section(|| config.header("Config file content:"))?
            .auth(host)
            .await
            .tap_ok(|auth| info!("inferred docker auth: {auth:?}"))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DockerConfig {
    /// The default credential store.
    ///
    /// The value of the config property is the suffix of the program to use (i.e. everything after `docker-credential-`).
    creds_store: Option<String>,

    /// Credential stores per host.
    ///
    /// Credential helpers are specified in a similar way to credsStore, but allow for multiple helpers to be configured at a time.
    /// Keys specify the registry domain, and values specify the suffix of the program to use (i.e. everything after docker-credential-).
    #[serde(default)]
    cred_helpers: HashMap<String, String>,

    /// Logged in hosts.
    #[serde(default)]
    auths: HashMap<String, DockerAuth>,
}

impl DockerConfig {
    /// Some hosts have fallback keys.
    /// Given a host, this function returns an iterator representing fallback keys to check for authentication.
    fn auth_keys<'a>(host: &'a str) -> impl Iterator<Item = &'a str> {
        if host == "docker.io" {
            vec!["docker.io", "https://index.docker.io/v1/"]
        } else {
            vec![host]
        }
        .into_iter()
    }

    /// Returns the auth for the host.
    ///
    /// Some hosts have fallback keys; the host that actually was used to retrieve the auth
    /// is returned so that if it was a fallback key the correct key can be used to
    /// retrieve auth information in subsequent operations.
    async fn auth(&self, host: &str) -> Result<Authentication> {
        for key in Self::auth_keys(host) {
            if let Some(auth) = self.auths.get(key) {
                match auth.decode(self, host).await {
                    Ok(auth) => return Ok(auth),
                    Err(err) => {
                        warn!("failed decoding auth for host {key}: {err}");
                        continue;
                    }
                }
            }
        }

        Ok(Authentication::None)
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DockerAuth {
    /// The credentials are stored in plain text, not in a helper.
    Plain {
        /// Base64 encoded authentication credentials in the form of `username:password`.
        auth: String,
    },

    /// The credentials are stored in a helper.
    /// Use the host with the top level [`DockerConfig`] to determine which helper to use.
    Helper {},
}

impl DockerAuth {
    async fn decode(&self, config: &DockerConfig, host: &str) -> Result<Authentication> {
        match self {
            DockerAuth::Plain { auth } => Self::decode_plain(auth),
            DockerAuth::Helper {} => Self::decode_helper(config, host).await,
        }
    }

    fn decode_plain(auth: &str) -> Result<Authentication> {
        let auth = base64::engine::general_purpose::STANDARD
            .decode(&auth)
            .context("decode base64 auth key")?;
        let auth = String::from_utf8(auth).context("parse auth key as utf-8")?;
        let (username, password) = auth
            .split_once(':')
            .ok_or_eyre("invalid auth key format, expected username:password")?;
        Ok(Authentication::basic(username, password))
    }

    async fn decode_helper(config: &DockerConfig, host: &str) -> Result<Authentication> {
        let helper = config
            .cred_helpers
            .get(host)
            .or(config.creds_store.as_ref())
            .ok_or_eyre("no helper found for host")?;

        let binary = format!("docker-credential-{helper}");
        let mut exec = tokio::process::Command::new(&binary)
            .arg("get")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawn docker credential helper")
            .with_section(|| binary.clone().header("Helper binary:"))?;

        if let Some(mut stdin) = exec.stdin.take() {
            stdin
                .write_all(host.as_bytes())
                .await
                .context("write request to helper")?;
            drop(stdin);
        }

        let output = exec.wait_with_output().await.context("run helper")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            return Err(eyre!("auth helper failed with status: {}", output.status))
                .with_section(|| binary.clone().header("Helper binary:"))
                .with_section(|| output.status.to_string().header("Command status code:"))
                .with_section(|| stderr.header("Stderr:"))
                .with_section(|| stdout.header("Stdout:"));
        }

        let credential = serde_json::from_slice::<DockerCredential>(&output.stdout)
            .context("decode helper output")
            .with_section(|| binary.header("Helper binary:"))?;
        Ok(Authentication::basic(
            credential.username,
            credential.secret,
        ))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerCredential {
    username: String,
    secret: String,
}
