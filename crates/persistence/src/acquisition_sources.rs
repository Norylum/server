//! Trusted source registry. No HTTP request is made by this module.

use crate::{AcquisitionLease, Database};
use chrono::{SecondsFormat, Utc};
use norylum_foundation::{AccountId, NorylumError};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use std::net::IpAddr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedAcquisitionSource {
    pub request_id: String,
    pub source_url: String,
    pub expected_sha256: String,
    /// `"https"` ou `"torrent"`. Pour les sources torrent, `source_url` est un magnet URI
    /// ou une URL .torrent, et `expected_sha256` est un placeholder non vérifié.
    pub source_type: String,
}

fn storage(error: impl std::fmt::Display) -> NorylumError {
    NorylumError::Internal(format!("Source acquisition SQLite : {error}"))
}

fn validation(message: &str) -> NorylumError {
    NorylumError::Validation(message.into())
}

fn valid_dns_name(value: &str) -> bool {
    if value.len() > 253
        || !value.contains('.')
        || value.parse::<IpAddr>().is_ok()
        || !value.is_ascii()
        || value.ends_with(".local")
        || value.ends_with(".localhost")
        || value.ends_with(".internal")
    {
        return false;
    }

    value.split('.').all(|part| {
        !part.is_empty()
            && part.len() <= 63
            && part.as_bytes()[0].is_ascii_alphanumeric()
            && part.as_bytes()[part.len() - 1].is_ascii_alphanumeric()
            && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

/// This is a registration gate, not yet a DNS rebinding defense.
/// Network fetching stays disabled until runtime IP checks exist.
fn validate_source(
    source_url: &str,
    sha256: &str,
    allowed_hosts: &[String],
) -> Result<(), NorylumError> {
    if source_url.len() > 2048
        || source_url
            .bytes()
            .any(|b| b.is_ascii_control() || b == b'\\')
    {
        return Err(validation("URL de source invalide"));
    }

    if sha256.len() != 64
        || !sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(validation("Empreinte SHA-256 invalide"));
    }

    let url =
        reqwest::Url::parse(source_url).map_err(|_| validation("URL de source mal formee"))?;

    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() == "/"
    {
        return Err(validation("Source HTTPS non autorisee"));
    }

    let host = url
        .host_str()
        .ok_or_else(|| validation("Domaine manquant"))?;

    if !valid_dns_name(host)
        || !allowed_hosts
            .iter()
            .any(|allowed| valid_dns_name(allowed) && host.eq_ignore_ascii_case(allowed))
    {
        return Err(validation("Domaine non autorise"));
    }

    Ok(())
}

impl Database {
    /// Registers one immutable source for an approved request.
    /// `allowed_hosts` must come from trusted administrator settings.
    pub async fn register_acquisition_source(
        &self,
        account_id: &AccountId,
        request_id: &str,
        source_url: &str,
        expected_sha256: &str,
        allowed_hosts: &[String],
    ) -> Result<(), NorylumError> {
        validate_source(source_url, expected_sha256, allowed_hosts)?;

        let account = account_id.as_str().to_owned();
        let id = request_id.to_owned();
        let url = source_url.to_owned();
        let sha256 = expected_sha256.to_owned();
        let now = Utc::now().to_rfc3339();

        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;

            let status: Option<String> = tx
                .query_row(
                    "SELECT status FROM acquisition_requests
                     WHERE id=?1 AND account_id=?2",
                    params![&id, &account],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;

            match status.as_deref() {
                None => {
                    return Err(NorylumError::NotFound("Demande introuvable".into()));
                }
                Some("approved") => {}
                Some(_) => {
                    return Err(NorylumError::Conflict(
                        "Demande non approuvee ou deja engagee".into(),
                    ));
                }
            }

            let changed = tx
                .execute(
                    "INSERT INTO acquisition_sources
                     (request_id, source_url, expected_sha256, registered_at)
                     VALUES (?1,?2,?3,?4)
                     ON CONFLICT DO NOTHING",
                    params![&id, &url, &sha256, &now],
                )
                .map_err(storage)?;

            if changed != 1 {
                return Err(NorylumError::Conflict(
                    "Une source est deja enregistree".into(),
                ));
            }

            tx.commit().map_err(storage)?;
            Ok(())
        })
        .await
    }

    /// Returns source details only to the current unexpired lease.
    /// Never expose this method through the public HTTP API.
    pub async fn source_for_acquisition_claim(
        &self,
        lease: &AcquisitionLease,
    ) -> Result<Option<AuthorizedAcquisitionSource>, NorylumError> {
        let id = lease.request_id.clone();
        let token = lease.lease_token.clone();
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);

        self.with_connection(|conn| {
            conn.query_row(
                "SELECT s.request_id, s.source_url, s.expected_sha256,
                        COALESCE(s.source_type, 'https')
                 FROM acquisition_sources s
                 JOIN acquisition_requests r ON r.id=s.request_id
                 JOIN acquisition_worker_leases l ON l.request_id=s.request_id
                 WHERE s.request_id=?1
                   AND l.lease_token=?2
                   AND l.expires_at > ?3
                   AND r.status='searching'",
                params![id, token, now],
                |row| {
                    Ok(AuthorizedAcquisitionSource {
                        request_id: row.get(0)?,
                        source_url: row.get(1)?,
                        expected_sha256: row.get(2)?,
                        source_type: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(storage)
        })
        .await
    }

    /// Enregistre une source BitTorrent (magnet URI ou URL .torrent) pour un bail actif.
    /// Réservé au worker : fencé par le token de bail.
    pub async fn register_torrent_source_via_lease(
        &self,
        lease: &AcquisitionLease,
        url: &str,
    ) -> Result<(), NorylumError> {
        if !url.starts_with("magnet:") && !url.starts_with("https://") {
            return Err(NorylumError::Validation(
                "Source torrent invalide : magnet: ou https:// requis".into(),
            ));
        }

        if url.len() > 4096 {
            return Err(NorylumError::Validation("Source torrent trop longue".into()));
        }

        let id = lease.request_id.clone();
        let token = lease.lease_token.clone();
        let url = url.to_owned();
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        // SHA-256 placeholder pour les sources torrent (l'intégrité est garantie par l'infohash).
        let placeholder = "0".repeat(64);

        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;

            // Vérifie que le bail est actif.
            let valid: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM acquisition_worker_leases l
                     JOIN acquisition_requests r ON r.id=l.request_id
                     WHERE l.request_id=?1
                       AND l.lease_token=?2
                       AND l.expires_at > ?3
                       AND r.status='searching'",
                    params![&id, &token, &now],
                    |row| row.get(0),
                )
                .map_err(storage)?;

            if valid == 0 {
                return Err(NorylumError::Conflict(
                    "Bail expiré ou invalide".into(),
                ));
            }

            let changed = tx
                .execute(
                    "INSERT INTO acquisition_sources
                     (request_id, source_url, expected_sha256, registered_at, source_type)
                     VALUES (?1,?2,?3,?4,'torrent')
                     ON CONFLICT DO NOTHING",
                    params![&id, &url, &placeholder, &now],
                )
                .map_err(storage)?;

            if changed != 1 {
                return Err(NorylumError::Conflict(
                    "Une source est déjà enregistrée".into(),
                ));
            }

            tx.commit().map_err(storage)?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use norylum_foundation::ProfileId;

    fn hosts() -> Vec<String> {
        vec!["cdn.example.org".to_owned()]
    }

    #[test]
    fn rejects_untrusted_and_malformed_sources() {
        let digest = "a".repeat(64);

        for bad in [
            "http://cdn.example.org/movie.mp4",
            "https://127.0.0.1/movie.mp4",
            "https://169.254.169.254/movie.mp4",
            "https://localhost/movie.mp4",
            "https://cdn.example.org.evil.invalid/movie.mp4",
            "https://user:password@cdn.example.org/movie.mp4",
            "https://cdn.example.org:8443/movie.mp4",
            "https://cdn.example.org/movie.mp4?token=secret",
            "https://cdn.example.org/movie.mp4#fragment",
            "https://cdn.example.org/",
            "file:///tmp/movie.mp4",
        ] {
            assert!(
                validate_source(bad, &digest, &hosts()).is_err(),
                "Source dangereuse acceptee : {bad}"
            );
        }

        assert!(validate_source("https://cdn.example.org/movie.mp4", &digest, &[],).is_err());

        assert!(
            validate_source("https://cdn.example.org/movie.mp4", "invalid", &hosts(),).is_err()
        );

        assert!(validate_source("https://cdn.example.org/movie.mp4", &digest, &hosts(),).is_ok());
    }

    #[tokio::test]
    async fn source_required_and_lease_token_fenced() {
        let db = Database::in_memory().unwrap();
        let account = AccountId::new("acc_source_test");
        let profile = ProfileId::new("prof_source_test");

        let request = db
            .request_acquisition(
                &account, &profile, "movie", 987650201, None, None, "1080p", "fr",
            )
            .await
            .unwrap();

        db.approve_acquisition_request(&account, &request.id)
            .await
            .unwrap();

        // Approval without an authorized source must not burn a retry.
        assert!(db
            .claim_next_approved_acquisition()
            .await
            .unwrap()
            .is_none());

        let other_account = AccountId::new("acc_other");

        assert!(db
            .register_acquisition_source(
                &other_account,
                &request.id,
                "https://cdn.example.org/movie.mp4",
                &"d".repeat(64),
                &hosts(),
            )
            .await
            .is_err());

        db.register_acquisition_source(
            &account,
            &request.id,
            "https://cdn.example.org/movie.mp4",
            &"d".repeat(64),
            &hosts(),
        )
        .await
        .unwrap();

        // Immutable source: cannot be silently replaced.
        assert!(db
            .register_acquisition_source(
                &account,
                &request.id,
                "https://cdn.example.org/other.mp4",
                &"e".repeat(64),
                &hosts(),
            )
            .await
            .is_err());

        let lease = db.claim_next_approved_acquisition().await.unwrap().unwrap();

        assert_eq!(lease.request_id, request.id);
        assert_eq!(lease.attempt, 1);

        let source = db
            .source_for_acquisition_claim(&lease)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(source.request_id, request.id);
        assert_eq!(source.source_url, "https://cdn.example.org/movie.mp4");

        let mut forged = lease.clone();
        forged.lease_token = "wrong-token".into();

        assert!(db
            .source_for_acquisition_claim(&forged)
            .await
            .unwrap()
            .is_none());

        // Expiration revokes access even before the next claim.
        db.with_connection(|conn| {
            conn.execute(
                "UPDATE acquisition_worker_leases
                 SET expires_at='2000-01-01T00:00:00.000Z'
                 WHERE request_id=?1",
                params![&lease.request_id],
            )
            .map_err(storage)?;
            Ok(())
        })
        .await
        .unwrap();

        assert!(db
            .source_for_acquisition_claim(&lease)
            .await
            .unwrap()
            .is_none());

        let next = db.claim_next_approved_acquisition().await.unwrap().unwrap();

        assert_eq!(next.request_id, request.id);
        assert_eq!(next.attempt, 2);
        assert_ne!(next.lease_token, lease.lease_token);

        assert!(db
            .source_for_acquisition_claim(&lease)
            .await
            .unwrap()
            .is_none());

        assert!(db
            .source_for_acquisition_claim(&next)
            .await
            .unwrap()
            .is_some());
    }
}
