//! Secrets in the desktop's keyring (the freedesktop Secret Service, such
//! as GNOME Keyring), never in ordinary configuration.
//!
//! An item is found by its attributes (`application` = `narrivy`, `name` =
//! the secret's name). The session uses the Secret Service's `plain`
//! algorithm: the value crosses only the user's own session bus. A locked
//! keyring is not unlocked from here (that needs the desktop's prompt);
//! the secret is then reported missing. Values are never logged.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zbus::zvariant::{OwnedObjectPath, Type, Value};

const SERVICE: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE_IFACE: &str = "org.freedesktop.Secret.Service";
const ITEM_IFACE: &str = "org.freedesktop.Secret.Item";
const COLLECTION_IFACE: &str = "org.freedesktop.Secret.Collection";
const DEFAULT_COLLECTION: &str = "/org/freedesktop/secrets/aliases/default";
const BOUND: Duration = Duration::from_secs(10);

/// Why a secret could not be read or kept. Content-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretError {
    /// No Secret Service on the session bus.
    Unavailable,
    /// The keyring is locked, or the desktop asked the user and was refused.
    Locked,
    /// The service answered with an error, or too slowly.
    Failed,
}

#[derive(Debug, Serialize, Deserialize, Type)]
struct Secret {
    session: OwnedObjectPath,
    parameters: Vec<u8>,
    value: Vec<u8>,
    content_type: String,
}

fn attributes(name: &str) -> HashMap<&str, &str> {
    HashMap::from([("application", "narrivy"), ("name", name)])
}

fn run<T>(work: impl std::future::Future<Output = Result<T, SecretError>>) -> Result<T, SecretError> {
    // `block_on` must not run inside another Tokio runtime.
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err(SecretError::Unavailable);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|_| SecretError::Unavailable)?;
    runtime.block_on(async {
        tokio::time::timeout(BOUND, work)
            .await
            .unwrap_or(Err(SecretError::Failed))
    })
}

async fn service(connection: &zbus::Connection) -> Result<zbus::Proxy<'static>, SecretError> {
    zbus::Proxy::new(connection, SERVICE, SERVICE_PATH, SERVICE_IFACE)
        .await
        .map_err(|_| SecretError::Unavailable)
}

async fn open_session(service: &zbus::Proxy<'_>) -> Result<OwnedObjectPath, SecretError> {
    let (_, session): (zbus::zvariant::OwnedValue, OwnedObjectPath) = service
        .call("OpenSession", &("plain", Value::from("")))
        .await
        .map_err(|_| SecretError::Unavailable)?;
    Ok(session)
}

/// The secret `name`, if the keyring has it.
///
/// # Errors
/// [`SecretError`] when the keyring cannot be reached or is locked.
pub fn read(name: &str) -> Result<Option<String>, SecretError> {
    run(async {
        let connection = zbus::Connection::session()
            .await
            .map_err(|_| SecretError::Unavailable)?;
        let service = service(&connection).await?;
        let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = service
            .call("SearchItems", &(attributes(name),))
            .await
            .map_err(|_| SecretError::Failed)?;
        let Some(item) = unlocked.into_iter().next() else {
            return if locked.is_empty() {
                Ok(None)
            } else {
                Err(SecretError::Locked)
            };
        };
        let session = open_session(&service).await?;
        let secrets: HashMap<OwnedObjectPath, Secret> = service
            .call("GetSecrets", &(vec![item.clone()], &session))
            .await
            .map_err(|_| SecretError::Failed)?;
        Ok(secrets
            .into_values()
            .next()
            .and_then(|secret| String::from_utf8(secret.value).ok()))
    })
}

/// Keep `value` as the secret `name` (replacing it), labelled for the
/// keyring's own window.
///
/// # Errors
/// [`SecretError`] when the keyring cannot be reached or is locked.
pub fn store(name: &str, label: &str, value: &str) -> Result<(), SecretError> {
    run(async {
        let connection = zbus::Connection::session()
            .await
            .map_err(|_| SecretError::Unavailable)?;
        let service = service(&connection).await?;
        let session = open_session(&service).await?;
        let collection = zbus::Proxy::new(&connection, SERVICE, DEFAULT_COLLECTION, COLLECTION_IFACE)
            .await
            .map_err(|_| SecretError::Unavailable)?;
        let mut properties: HashMap<&str, Value<'_>> = HashMap::new();
        properties.insert("org.freedesktop.Secret.Item.Label", Value::from(label));
        properties.insert(
            "org.freedesktop.Secret.Item.Attributes",
            Value::from(attributes(name)),
        );
        let secret = Secret {
            session,
            parameters: Vec::new(),
            value: value.as_bytes().to_vec(),
            content_type: "text/plain; charset=utf8".to_owned(),
        };
        let (_, prompt): (OwnedObjectPath, OwnedObjectPath) = collection
            .call("CreateItem", &(properties, secret, true))
            .await
            .map_err(|_| SecretError::Failed)?;
        // A prompt means the keyring is locked: not unlocked from here.
        if prompt.as_str() != "/" {
            return Err(SecretError::Locked);
        }
        Ok(())
    })
}

/// Remove the secret `name` from the keyring.
///
/// # Errors
/// [`SecretError`] when the keyring cannot be reached or is locked.
pub fn remove(name: &str) -> Result<(), SecretError> {
    run(async {
        let connection = zbus::Connection::session()
            .await
            .map_err(|_| SecretError::Unavailable)?;
        let service = service(&connection).await?;
        let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = service
            .call("SearchItems", &(attributes(name),))
            .await
            .map_err(|_| SecretError::Failed)?;
        if !locked.is_empty() {
            return Err(SecretError::Locked);
        }
        for item in unlocked {
            let proxy = zbus::Proxy::new(&connection, SERVICE, item, ITEM_IFACE)
                .await
                .map_err(|_| SecretError::Failed)?;
            let _: OwnedObjectPath = proxy
                .call("Delete", &())
                .await
                .map_err(|_| SecretError::Failed)?;
        }
        Ok(())
    })
}
