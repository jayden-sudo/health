//! The profile's `vault` (plan §3.2): the health account's AMK and the state tied to it, sealed with a key
//! derived from the root key. Not in `health.db` (its schema is fixed) and not in backups: an account moves
//! between devices only by linking (protocol 3.5).

use crate::error::{Error, Result};
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use zeroize::Zeroizing;

const AAD: &[u8] = b"health-vault-v1";

/// No Debug: it holds the account key.
#[derive(Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct State {
    /// base64url of the 32-byte AMK
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amk: Option<String>,
    /// The inbox key registered with the backend (base64url); the identity code is shown only for it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registered_pk: Option<String>,
    /// Per inbox key: the last `seq` handled
    #[serde(default)]
    pub cursors: BTreeMap<String, i64>,
    /// Per inbox key: results discarded (shown to the person, protocol 2.3)
    #[serde(default)]
    pub drops: BTreeMap<String, i64>,
    /// The personal profile (web `profile.ts`): kept here, on this device only, as the web keeps it outside its
    /// export. Height and weight aren't here: they are the latest readings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Personal>,
    /// Link history (owner, 2026-11-22: a local record only, as the web's link-log): per account id, every link
    /// this device completed. After an adopt replaces the account, the old account's entries stay here, not
    /// shown and not deletable: on purpose (the record is of this device, not of the current account).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub links: BTreeMap<String, Vec<LinkEntry>>,
    /// The WHOOP connection (the person's own developer app and its tokens)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whoop: Option<Whoop>,
}

/// One completed link (web `link-log.ts` LinkEntry).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, schemars::JsonSchema)]
pub struct LinkEntry {
    /// 8 random bytes, base64url
    pub id: String,
    /// "gave" (this device handed the account over) | "got" (this device took it)
    pub dir: String,
    /// When (ms)
    pub at: i64,
    /// The check code both devices showed ("1234 5678")
    pub check: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, schemars::JsonSchema)]
pub struct Personal {
    /// YYYY-MM
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub birth: Option<String>,
    /// "Male" | "Female"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gender: Option<String>,
    /// For seca self-service only (asked there, not in the profile page)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ethnicity: Option<String>,
}

#[derive(Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct Whoop {
    pub client_id: String,
    pub client_secret: String,
    /// The `state` of a sign-in not yet back
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh: Option<String>,
    /// When the access token expires (ms)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<i64>,
    /// The refresh token stopped working: sign in again
    #[serde(default)]
    pub reauth: bool,
}

pub struct Vault {
    path: PathBuf,
    tmp: PathBuf,
    key: Zeroizing<[u8; 32]>,
}

impl Vault {
    pub fn new(path: PathBuf, tmp: PathBuf, key: [u8; 32]) -> Vault {
        Vault { path, tmp, key: Zeroizing::new(key) }
    }

    pub fn load(&self) -> Result<State> {
        let raw = match std::fs::read(&self.path) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(State::default()),
            Err(e) => return Err(e.into()),
        };
        if raw.len() < 12 + 16 {
            return Err(Error::other("vault", "the vault file is damaged"));
        }
        let cipher = Aes256Gcm::new((&*self.key).into());
        let nonce: [u8; 12] = raw[..12].try_into().expect("12");
        let plain = Zeroizing::new(
            cipher
                .decrypt(&Nonce::from(nonce), Payload { msg: &raw[12..], aad: AAD })
                .map_err(|_| Error::other("vault", "the vault file is damaged or from another key"))?,
        );
        serde_json::from_slice(&plain).map_err(|_| Error::other("vault", "the vault file is damaged"))
    }

    pub fn save(&self, s: &State) -> Result<()> {
        let plain = Zeroizing::new(serde_json::to_vec(s)?);
        let nonce = crate::aead::random::<12>();
        let cipher = Aes256Gcm::new((&*self.key).into());
        let sealed = cipher.encrypt(&Nonce::from(nonce), Payload { msg: &plain, aad: AAD }).expect("AES-GCM encrypt");
        let tmp = self.tmp.join(format!("vault-{}", hex::encode(crate::aead::random::<8>())));
        {
            use std::io::Write;
            let mut f = crate::profile::create_private(&tmp)?;
            f.write_all(&nonce)?;
            f.write_all(&sealed)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Loads, changes and saves the state.
    pub fn update<T>(&self, f: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        let mut s = self.load()?;
        let out = f(&mut s)?;
        self.save(&s)?;
        Ok(out)
    }
}
