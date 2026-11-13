//! which origins each secret may be sent to
//!
//! an origin is scheme, host and port, written one way only: lower case
//! and the port always spelled out, so `https://API.example.com` and
//! `https://api.example.com:443` are the same approved origin, while
//! `http://` or another port is a different one

use super::{SecretError, SecretIndex, SecretKey, Secrets};
use crate::http::RedirectRule;
use crate::url::{Origin, origin};

/// the one written form of `url`'s origin, such as
/// `https://api.example.com:443`, or `None` when it has no scheme or host
///
/// ```
/// use curst_core::secrets::normal_origin;
///
/// assert_eq!(
///     normal_origin("https://API.example.com/v1").as_deref(),
///     Some("https://api.example.com:443")
/// );
/// assert_eq!(normal_origin("api.example.com/v1"), None);
/// ```
#[must_use]
pub fn normal_origin(url: &str) -> Option<String> {
    origin(url).map(|o| written(&o))
}

fn written(origin: &Origin) -> String {
    match origin.port {
        Some(port) => format!("{}://{}:{port}", origin.scheme, origin.host),
        None => format!("{}://{}", origin.scheme, origin.host),
    }
}

/// secrets a request would carry to an origin they are not approved for
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    /// the origin, in its written form
    pub origin: String,

    /// the secrets not yet approved for it
    pub keys: Vec<SecretKey>,
}

impl SecretIndex {
    /// what needs approving before `keys` may be sent to `url`, or `None`
    /// when every one of them is approved for its origin
    ///
    /// # Errors
    /// [`SecretError::NoOrigin`] when `url` has no scheme and host, since a
    /// secret is never sent without knowing where to; or when the index
    /// cannot be read
    pub fn unapproved(
        &self,
        keys: &[SecretKey],
        url: &str,
    ) -> Result<Option<Approval>, SecretError> {
        let origin = normal_origin(url).ok_or_else(|| SecretError::NoOrigin(url.to_owned()))?;
        let entries = self.entries()?;
        let missing: Vec<SecretKey> = keys
            .iter()
            .filter(|key| {
                !entries
                    .iter()
                    .any(|e| e.key == **key && e.hosts.contains(&origin))
            })
            .cloned()
            .collect();

        Ok((!missing.is_empty()).then_some(Approval {
            origin,
            keys: missing,
        }))
    }

    /// records that `keys` may be sent to `origin`
    ///
    /// # Errors
    /// [`SecretError::NotFound`] for a key with no stored secret, or when
    /// the index cannot be written
    pub fn approve(&self, keys: &[SecretKey], origin: &str) -> Result<(), SecretError> {
        let origin =
            normal_origin(origin).ok_or_else(|| SecretError::NoOrigin(origin.to_owned()))?;

        self.locked(|locked| {
            for key in keys {
                let entry = locked
                    .entries
                    .iter_mut()
                    .find(|e| e.key == *key)
                    .ok_or_else(|| SecretError::NotFound(key.to_string()))?;

                if !entry.hosts.contains(&origin) {
                    entry.hosts.push(origin.clone());
                }
            }

            locked.save()
        })
    }

    /// the redirects a request carrying `keys` may follow: any, without
    /// secrets; otherwise only to origins approved for every one of them
    ///
    /// # Errors
    /// when the index cannot be read
    pub fn redirect_rule(&self, keys: &[SecretKey]) -> Result<RedirectRule, SecretError> {
        if keys.is_empty() {
            return Ok(RedirectRule::Any);
        }

        let entries = self.entries()?;
        let hosts_of = |key: &SecretKey| {
            entries
                .iter()
                .find(|e| e.key == *key)
                .map_or_else(Vec::new, |e| e.hosts.clone())
        };
        let mut shared = hosts_of(&keys[0]);

        for key in &keys[1..] {
            let hosts = hosts_of(key);

            shared.retain(|h| hosts.contains(h));
        }

        Ok(RedirectRule::Within(
            shared.iter().filter_map(|h| origin(h)).collect(),
        ))
    }
}

impl Secrets {
    /// the index, which also records the origins each secret may go to
    #[must_use]
    pub fn index(&self) -> &SecretIndex {
        &self.index
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::model::LocalId;
    use crate::secrets::{MemoryStore, Secret, SecretIndex};
    use crate::vars::VarName;

    fn setup() -> (tempfile::TempDir, Secrets) {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Secrets::new(
            SecretIndex::new(dir.path().join("secrets-index.toml")),
            Arc::new(MemoryStore::new()),
        );

        (dir, secrets)
    }

    fn global(name: &str) -> SecretKey {
        SecretKey::Global {
            name: VarName::new(name).unwrap(),
        }
    }

    #[test]
    fn origins_have_one_written_form() {
        assert_eq!(
            normal_origin("https://API.example.com"),
            normal_origin("https://api.example.com:443/other?x")
        );
        assert_eq!(normal_origin("http://h/").as_deref(), Some("http://h:80"));
        assert_eq!(normal_origin("ftp://h/").as_deref(), Some("ftp://h"));
        assert_eq!(
            normal_origin("http://[::1]:8080/").as_deref(),
            Some("http://[::1]:8080")
        );
        assert_ne!(normal_origin("http://h/"), normal_origin("https://h/"));
        assert_ne!(
            normal_origin("https://h/"),
            normal_origin("https://h:8443/")
        );
    }

    #[test]
    fn a_first_send_and_every_new_origin_ask() {
        let (_dir, secrets) = setup();
        let token = global("token");

        secrets.set(&token, &Secret::new("t")).unwrap();

        let ask = secrets
            .index()
            .unapproved(std::slice::from_ref(&token), "https://API.example.com/v1")
            .unwrap()
            .unwrap();

        assert_eq!(ask.origin, "https://api.example.com:443");
        assert_eq!(ask.keys, std::slice::from_ref(&token));

        secrets
            .index()
            .approve(std::slice::from_ref(&token), "https://api.example.com")
            .unwrap();

        // the same origin spelled another way needs nothing more
        assert_eq!(
            secrets
                .index()
                .unapproved(
                    std::slice::from_ref(&token),
                    "https://api.example.com:443/v2"
                )
                .unwrap(),
            None
        );

        // plain HTTP, another port or another host each ask again
        for url in [
            "http://api.example.com/v1",
            "https://api.example.com:8443/v1",
            "https://evil.example.com/v1",
        ] {
            assert!(
                secrets
                    .index()
                    .unapproved(std::slice::from_ref(&token), url)
                    .unwrap()
                    .is_some(),
                "{url}"
            );
        }

        assert!(matches!(
            secrets
                .index()
                .unapproved(std::slice::from_ref(&token), "api.example.com"),
            Err(SecretError::NoOrigin(_))
        ));
        assert_eq!(secrets.index().unapproved(&[], "https://x/").unwrap(), None);
    }

    #[test]
    fn approvals_are_recorded_once_and_need_a_secret() {
        let (_dir, secrets) = setup();
        let token = global("token");

        secrets.set(&token, &Secret::new("t")).unwrap();
        secrets
            .index()
            .approve(std::slice::from_ref(&token), "https://h/")
            .unwrap();
        secrets
            .index()
            .approve(std::slice::from_ref(&token), "https://H:443/")
            .unwrap();

        assert_eq!(secrets.list().unwrap()[0].hosts, ["https://h:443"]);
        assert!(matches!(
            secrets.index().approve(&[global("other")], "https://h/"),
            Err(SecretError::NotFound(_))
        ));
        assert!(matches!(
            secrets.index().approve(&[token], "h"),
            Err(SecretError::NoOrigin(_))
        ));
    }

    #[test]
    fn a_renamed_secret_keeps_its_origins() {
        let (_dir, secrets) = setup();
        let old = SecretKey::Collection {
            collection: LocalId::new("c").unwrap(),
            name: VarName::new("old").unwrap(),
        };
        let new = old.renamed(VarName::new("new").unwrap());

        secrets.set(&old, &Secret::new("t")).unwrap();
        secrets
            .index()
            .approve(std::slice::from_ref(&old), "https://h/")
            .unwrap();
        secrets.rename(&old, &new).unwrap();

        assert_eq!(
            secrets.index().unapproved(&[new], "https://h/").unwrap(),
            None
        );
    }

    #[test]
    fn redirects_stay_where_every_secret_is_approved() {
        let (_dir, secrets) = setup();
        let a = global("a");
        let b = global("b");

        secrets.set(&a, &Secret::new("1")).unwrap();
        secrets.set(&b, &Secret::new("2")).unwrap();
        secrets
            .index()
            .approve(&[a.clone(), b.clone()], "https://shared/")
            .unwrap();
        secrets
            .index()
            .approve(std::slice::from_ref(&a), "https://only-a/")
            .unwrap();

        assert_eq!(
            secrets.index().redirect_rule(&[]).unwrap(),
            RedirectRule::Any
        );

        let rule = secrets.index().redirect_rule(&[a.clone(), b]).unwrap();

        assert!(rule.allows("https://shared/next"));
        assert!(!rule.allows("https://only-a/next"));
        assert!(!rule.allows("http://shared/next"));
        assert!(
            secrets
                .index()
                .redirect_rule(&[a])
                .unwrap()
                .allows("https://only-a/x")
        );
        assert!(
            !secrets
                .index()
                .redirect_rule(&[global("unknown")])
                .unwrap()
                .allows("https://shared/")
        );
    }
}
