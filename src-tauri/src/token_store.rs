//! Protection for secrets stored on disk.
//!
//! On Windows the tokens live in the Windows Credential Manager (via the
//! `keyring` crate) instead of the account JSON files; the files only keep a
//! `wcm:<key>` reference to the credential. The Credential Manager seals its
//! contents with DPAPI bound to the current user, and the tokens no longer
//! travel with the account files at all. Other platforms keep the historical
//! plaintext behaviour.

/// Marker prepended to stored values that are only a reference into the OS
/// credential store, so legacy values are never mistaken for a reference.
const KEYRING_PREFIX: &str = "wcm:";

/// Service name grouping Quota's credentials in the Windows Credential Manager.
const KEYRING_SERVICE: &str = "dev.pinkpixel.quota";

#[cfg(target_os = "windows")]
fn entry(key: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, key)
        .map_err(|e| format!("Could not open the Windows Credential Manager: {e}"))
}

/// Stores a secret in the OS credential store under `key` and returns the
/// reference to write into the account file instead of the secret itself.
pub fn seal(key: &str, value: &str) -> Result<String, String> {
    seal_impl(key, value)
}

/// Recovers a secret previously stored by [`seal`]. Values without the
/// keyring prefix are legacy plaintext (or DPAPI-sealed) and returned
/// unchanged so existing installs keep working until their next save.
pub fn unseal(stored: &str) -> Result<String, String> {
    match stored.strip_prefix(KEYRING_PREFIX) {
        Some(key) => unseal_impl(key),
        None => Ok(stored.to_string()),
    }
}

/// True when the stored value is only a keyring reference.
pub fn is_sealed(stored: &str) -> bool {
    stored.starts_with(KEYRING_PREFIX)
}

/// Removes a stored credential. Best effort: a missing entry is fine.
pub fn remove(stored: &str) {
    let Some(key) = stored.strip_prefix(KEYRING_PREFIX) else {
        return;
    };
    #[cfg(target_os = "windows")]
    {
        if let Ok(entry) = entry(key) {
            let _ = entry.delete_credential();
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = key;
}

#[cfg(target_os = "windows")]
fn seal_impl(key: &str, value: &str) -> Result<String, String> {
    entry(key)?
        .set_password(value)
        .map_err(|e| format!("Could not store the token in the Windows Credential Manager: {e}"))?;
    Ok(format!("{KEYRING_PREFIX}{key}"))
}

#[cfg(target_os = "windows")]
fn unseal_impl(key: &str) -> Result<String, String> {
    entry(key)?.get_password().map_err(|e| {
        format!("Could not read the token from the Windows Credential Manager: {e}")
    })
}

#[cfg(not(target_os = "windows"))]
fn seal_impl(_key: &str, value: &str) -> Result<String, String> {
    Ok(value.to_string())
}

#[cfg(not(target_os = "windows"))]
fn unseal_impl(key: &str) -> Result<String, String> {
    // Not reachable in practice: seal_impl on these platforms never writes the
    // prefix, so unseal() returns before calling this. Present for completeness.
    Ok(key.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    fn keyring_tokens_round_trip() {
        let key = format!("quota-test-{}", std::process::id());
        let original = "eyJhbGciOi.fake-cursor-token.signature&with=symbols";
        let stored = seal(&key, original).expect("sealing should succeed");
        assert!(
            is_sealed(&stored),
            "sealed values must carry the prefix, got: {stored}"
        );
        assert!(
            !stored.contains(original),
            "the plaintext token must not appear in the stored value"
        );
        assert_eq!(unseal(&stored).unwrap(), original);
        remove(&stored);
        assert!(
            unseal(&stored).is_err(),
            "the credential must be gone after remove()"
        );
    }

    #[test]
    fn legacy_plaintext_is_returned_unchanged() {
        assert_eq!(unseal("plain-legacy-token").unwrap(), "plain-legacy-token");
        assert!(!is_sealed("plain-legacy-token"));
    }
}
