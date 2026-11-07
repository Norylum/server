use super::*;

fn temp_quarantine() -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("norylum-transport-test-{}", uuid::Uuid::new_v4()));

    std::fs::create_dir(&root).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    root
}

fn fixture(url: &str) -> AuthorizedAcquisitionSource {
    AuthorizedAcquisitionSource {
        request_id: "acq_test".to_owned(),
        source_url: url.to_owned(),
        expected_sha256: "a".repeat(64),
        source_type: "https".to_owned(),
    }
}

#[test]
fn rejects_private_reserved_and_documentation_ips() {
    for address in [
        "0.0.0.0",
        "10.0.0.1",
        "100.64.0.1",
        "127.0.0.1",
        "169.254.169.254",
        "172.16.0.1",
        "192.0.0.1",
        "192.0.2.1",
        "192.168.1.1",
        "198.18.0.1",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "255.255.255.255",
    ] {
        let ip: Ipv4Addr = address.parse().unwrap();
        assert!(!is_public_ipv4(ip), "IP acceptee : {ip}");
    }

    assert!(is_public_ipv4("8.8.8.8".parse().unwrap()));
}

#[test]
fn rejects_untrusted_urls_before_any_network() {
    let hosts = vec!["cdn.example.org".to_owned()];

    for raw in [
        "http://cdn.example.org/movie.mp4",
        "https://127.0.0.1/movie.mp4",
        "https://localhost/movie.mp4",
        "https://cdn.example.org.evil.invalid/movie.mp4",
        "https://user:pass@cdn.example.org/movie.mp4",
        "https://cdn.example.org:8443/movie.mp4",
        "https://cdn.example.org/movie.mp4?token=x",
        "https://cdn.example.org/movie.mp4#part",
    ] {
        assert!(
            validate_url(&fixture(raw), &hosts).is_err(),
            "URL acceptee : {raw}"
        );
    }

    let allowed = fixture("https://cdn.example.org/movie.mp4");

    assert!(validate_url(&allowed, &[]).is_err());
    assert!(validate_url(&allowed, &hosts).is_ok());
}

#[tokio::test]
async fn verified_file_is_quarantined_and_deleted_on_drop() {
    let dir = temp_quarantine();

    let expected = format!("{:x}", Sha256::digest(b"hello world"));

    let mut sink = VerifiedSink::create(&dir, &expected, 1024).unwrap();

    sink.write_chunk(b"hello ").await.unwrap();
    sink.write_chunk(b"world").await.unwrap();

    let artifact = sink.finish().await.unwrap();
    assert_eq!(artifact.bytes(), 11);

    let path = artifact.path().to_owned();

    assert_eq!(std::fs::read(&path).unwrap(), b"hello world");

    assert!(path.starts_with(std::fs::canonicalize(&dir).unwrap()));

    drop(artifact);
    assert!(!path.exists());

    std::fs::remove_dir(&dir).unwrap();
}

#[tokio::test]
async fn oversized_transfer_leaves_no_partial_file() {
    let dir = temp_quarantine();
    let expected = "a".repeat(64);

    let mut sink = VerifiedSink::create(&dir, &expected, 3).unwrap();

    assert!(matches!(
        sink.write_chunk(b"1234").await,
        Err(TransferError::SizeLimit)
    ));

    drop(sink);

    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    std::fs::remove_dir(&dir).unwrap();
}

#[tokio::test]
async fn invalid_digest_leaves_no_partial_file() {
    let dir = temp_quarantine();

    let mut sink = VerifiedSink::create(&dir, &"a".repeat(64), 1024).unwrap();

    sink.write_chunk(b"wrong bytes").await.unwrap();

    assert!(matches!(
        sink.finish().await,
        Err(TransferError::DigestMismatch)
    ));

    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    std::fs::remove_dir(&dir).unwrap();
}

#[tokio::test]
async fn quarantine_rejects_symlinked_directory() {
    let dir = temp_quarantine();
    let expected = "a".repeat(64);

    #[cfg(unix)]
    {
        let link = dir.with_extension("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();

        assert!(matches!(
            VerifiedSink::create(&link, &expected, 1024),
            Err(TransferError::InvalidSource)
        ));

        std::fs::remove_file(link).unwrap();
    }

    std::fs::remove_dir(dir).unwrap();
}
