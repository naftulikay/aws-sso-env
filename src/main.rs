use anyhow::{anyhow, Result};

use aws_sdk_sso::{Client as SsoClient, Config as SsoConfig, Region as SsoRegion};
use aws_types::os_shim_internal::{Env, Fs};

use log::LevelFilter;

use serde::Deserialize;

use sha1::Sha1;

use structopt::StructOpt;

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use tokio;

use zeroize::Zeroize;

/// Extract and export AWS environment variables for a specified SSO profile.
#[derive(Debug, StructOpt)]
pub struct Args {
    /// The name of an SSO profile in your local AWS configuration file(s).
    pub profile_name: String,
}

/// Representation of an SSO profile's configuration within `~/.aws/config` or `~/.aws/credentials`.
///
/// This struct contains all the necessary fields to facilitate single-sign-on for an AWS account with a role.
#[derive(Debug)]
pub struct SsoProfile {
    pub profile_name: String,
    pub region: String,
    pub sso_account_id: String,
    pub sso_region: String,
    pub sso_role_name: String,
    pub sso_start_url: String,
}

#[derive(Debug, Deserialize, Zeroize)]
#[serde(rename_all = "camelCase")]
pub struct CachedSsoToken {
    pub access_token: String,
    pub expires_at: String,
    pub region: String,
    pub start_url: String,
}

#[derive(Debug, Zeroize)]
pub struct SsoCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    #[zeroize(skip)]
    pub expires_at: OffsetDateTime,
}

impl CachedSsoToken {
    pub fn expires_at(&self) -> Result<OffsetDateTime> {
        OffsetDateTime::parse(self.expires_at.as_str(), &Rfc3339)
            .map_err(|e| anyhow!("unable to parse date-time: {:?}", e))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::builder()
        .filter("h2".into(), LevelFilter::Error)
        .filter("rustls".into(), LevelFilter::Error)
        .filter("hyper".into(), LevelFilter::Error)
        .filter("tracing".into(), LevelFilter::Error)
        .filter("aws_smithy_client".into(), LevelFilter::Error)
        .filter("aws_smithy_http_tower".into(), LevelFilter::Error)
        .filter("aws_http".into(), LevelFilter::Error)
        .filter("aws_endpoint".into(), LevelFilter::Error)
        .filter("aws_config".into(), LevelFilter::Error)
        .filter_level(LevelFilter::Debug)
        .init();

    let args = Args::from_args();
    let profile_name: String = args.profile_name;

    // first, load the SSO configuration for the given profile
    let sso_profile = get_sso_profile(profile_name.as_str()).await?;

    log::debug!("Found SSO profile: {:#?}", sso_profile);

    // next, see if there is a cached SSO token available in the cached tokens directory
    if let Some(cached_sso_token) = load_cached_token(&sso_profile).await {
        log::debug!("Loaded cached SSO token.");

        if let Ok(expires_at) = cached_sso_token.expires_at() {
            let encoded = expires_at.format(&Rfc3339)?;

            if OffsetDateTime::now_utc() > expires_at {
                log::error!("Cached SSO token is expired as of {}", encoded);
                log::info!(
                    "Run 'aws --profile {} sso login' to refresh credentials.",
                    profile_name
                );
                return Ok(());
            }

            log::debug!("Cached SSO token is still valid, expires at {}", encoded);

            // finally, use the sso client to fetch credentials
            let credentials = fetch_sso_credentials(&sso_profile, &cached_sso_token)
                .await
                .map_err(|e| {
                    log::error!(
                        "Unable to fetch SSO credentials using cached SSO token: {:?}",
                        e
                    );
                    e
                })?;

            log::info!("Obtained SSO credentials, printing to standard output:");

            println!("# expires at {}", encoded);
            println!("export AWS_ACCESS_KEY_ID={}", credentials.access_key_id);
            println!(
                "export AWS_SECRET_ACCESS_KEY={}",
                credentials.secret_access_key
            );
            println!("export AWS_SESSION_TOKEN={}", credentials.session_token);
        }
    }

    Ok(())
}

async fn get_sso_profile<S: AsRef<str>>(profile_name: S) -> Result<SsoProfile> {
    // use the default filesystem and the default environment variables
    let (fs, env) = (Fs::default(), Env::default());

    // load the profile set from disk
    let profiles = aws_config::profile::load(&fs, &env)
        .await
        .map_err(|e| anyhow!("unable to get profiles: {}", e))?;

    // get the profile with the given name
    //
    // NOTE the sdk does not allow you to list profiles, which is an interesting choice, you have to _know_ what
    //      profile you're looking for
    if let Some(profile) = profiles.get_profile(profile_name.as_ref()) {
        // extract all the properties, converting them to errors if not present
        Ok(SsoProfile {
            profile_name: profile_name.as_ref().into(),
            region: profile
                .get("region")
                .ok_or(anyhow!("profile must have region property set"))?
                .into(),
            sso_account_id: profile
                .get("sso_account_id")
                .ok_or(anyhow!("profile must have sso_account_id property set"))?
                .into(),
            sso_region: profile
                .get("sso_region")
                .ok_or(anyhow!("profile must have sso_region property set"))?
                .into(),
            sso_role_name: profile
                .get("sso_role_name")
                .ok_or(anyhow!("profile must have sso_role_name property set"))?
                .into(),
            sso_start_url: profile
                .get("sso_start_url")
                .ok_or(anyhow!("profile must have sso_start_url property set"))?
                .into(),
        })
    } else {
        // the profile was not found
        Err(anyhow!("profile '{}' not found", profile_name.as_ref()))
    }
}

async fn load_cached_token(sso_profile: &SsoProfile) -> Option<CachedSsoToken> {
    let cache_dir = dirs::home_dir()
        .expect("unable to get the current user's home dir")
        .join(".aws")
        .join("sso")
        .join("cache");

    if !cache_dir.is_dir() {
        log::debug!(
            "SSO credentials cache directory does not exist: {}",
            cache_dir.display()
        );
        return None;
    }

    let cache_filename = format!(
        "{}.json",
        Sha1::from(sso_profile.sso_start_url.as_str()).hexdigest()
    );

    let cache_file = cache_dir.join(cache_filename);

    if !cache_file.is_file() {
        log::debug!(
            "Cache file for profile '{}' does not exist.",
            sso_profile.profile_name
        );
        return None;
    }

    tokio::fs::read_to_string(cache_file)
        .await
        .map(|s| {
            serde_json::from_str::<CachedSsoToken>(s.as_str())
                .map_err(|e| log::error!("Unable to deserialize cached SSO token: {:?}", e))
                .ok()
        })
        .ok()
        .flatten()
}

async fn fetch_sso_credentials(
    profile: &SsoProfile,
    token: &CachedSsoToken,
) -> Result<SsoCredentials> {
    let config = SsoConfig::builder()
        .region(SsoRegion::new(token.region.clone()))
        .build();

    let client = SsoClient::from_conf(config);

    let role_credentials = client
        .get_role_credentials()
        .account_id(profile.sso_account_id.clone())
        .role_name(profile.sso_role_name.clone())
        .access_token(token.access_token.clone())
        .send()
        .await?
        .role_credentials
        .ok_or(anyhow!("response did not contain any credentials"))?;

    Ok(SsoCredentials {
        access_key_id: role_credentials
            .access_key_id
            .ok_or(anyhow!("response did not contain an access key id"))?,
        secret_access_key: role_credentials
            .secret_access_key
            .ok_or(anyhow!("response did not contain a secret access key"))?,
        session_token: role_credentials
            .session_token
            .ok_or(anyhow!("response did not contain a session token"))?,
        expires_at: OffsetDateTime::from_unix_timestamp_nanos(role_credentials.expiration.into())
            .map_err(|e| {
            anyhow!(
                "unable to parse expiration date from role credentials: {:?}",
                e
            )
        })?,
    })
}
