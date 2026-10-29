//! Opt-in real protocol tests against tests/quicklook/compose.yml. No real
//! credentials or existing user sessions are used.
use super::{
    staging::{self, Source, StagingRoot},
    MAX_BYTES,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

type Handle = Arc<Mutex<russh::client::Handle<crate::ssh::handler::SshClientHandler>>>;

async fn connect(port: u16) -> Handle {
    let config = Arc::new(russh::client::Config::default());
    let mut handle = russh::client::connect(
        config,
        ("127.0.0.1", port),
        crate::ssh::handler::SshClientHandler,
    )
    .await
    .unwrap();
    assert!(handle
        .authenticate_password("testuser", "testpass")
        .await
        .unwrap());
    Arc::new(Mutex::new(handle))
}

#[tokio::test]
#[ignore = "requires loopback SSH/SCP fixture containers from tests/quicklook/compose.yml"]
async fn quicklook_real_ssh_adapters() {
    for (port, is_sftp, expected_flavor) in [
        (24222, true, crate::scp::listing::Flavor::Gnu),
        (24223, false, crate::scp::listing::Flavor::Gnu),
        (24224, false, crate::scp::listing::Flavor::Busybox),
    ] {
        let handle = connect(port).await;
        let flavor = crate::scp::exec::detect_flavor(handle.clone())
            .await
            .unwrap();
        assert_eq!(flavor, expected_flavor);
        let remote_dir = format!("/tmp/anyscp-preview-{}", uuid::Uuid::new_v4());
        let cmd = format!("mkdir {}", crate::scp::shell_quote(&remote_dir));
        crate::scp::exec::ssh_exec(handle.clone(), &cmd)
            .await
            .unwrap();
        let parent = tempfile::tempdir().unwrap();
        let staging = StagingRoot::new(parent.path()).unwrap();
        let source = |path: String| {
            if is_sftp {
                Source::Sftp {
                    handle: handle.clone(),
                    sudo: false,
                    path,
                    ssh_id: "fixture".into(),
                }
            } else {
                Source::Scp {
                    handle: handle.clone(),
                    flavor,
                    path,
                    ssh_id: "fixture".into(),
                }
            }
        };
        let token = CancellationToken::new();
        let scope = crate::ssh::channel_scope::ChannelScope::new(&token);
        for (name, contents) in [
            ("Unicode 測試.txt", b"preview text\n".as_slice()),
            (
                "image.png",
                include_bytes!("../../icons/32x32.png").as_slice(),
            ),
            ("document.pdf", b"%PDF-1.4\nfixture\n%%EOF\n".as_slice()),
            ("empty.txt", b"".as_slice()),
        ] {
            let local = parent.path().join(name);
            std::fs::write(&local, contents).unwrap();
            let remote = format!("{remote_dir}/{name}");
            crate::scp::transfer::upload_file(handle.clone(), &local, &remote, &token, |_| {})
                .await
                .unwrap();
            let file = staging.request(name).unwrap();
            staging::download(&source(remote), &file, &token, &scope, |_, _| {})
                .await
                .unwrap();
            assert_eq!(std::fs::read(&file.complete).unwrap(), contents);
        }
        let base = crate::scp::shell_quote(&remote_dir);
        let command = format!("ln -s {base}/image.png {base}/link.png; mkdir {base}/folder; touch {base}/denied.txt; chmod 000 {base}/denied.txt; truncate -s {} {base}/large.txt", MAX_BYTES + 1);
        // busybox's truncate is also supported by the fixture image.
        assert_eq!(
            crate::scp::exec::ssh_exec(handle.clone(), &command)
                .await
                .unwrap()
                .2,
            0
        );
        for (name, kind) in [
            ("link.png", "invalid_selection"),
            ("folder", "invalid_selection"),
            ("missing.txt", "download_failed"),
            ("denied.txt", "download_failed"),
            ("large.txt", "too_large"),
        ] {
            let file = staging.request(name).unwrap();
            let error = staging::download(
                &source(format!("{remote_dir}/{name}")),
                &file,
                &token,
                &scope,
                |_, _| {},
            )
            .await
            .unwrap_err();
            assert_eq!(error.kind, kind, "port {port}, {name}: {error}");
            assert!(!file.complete.exists());
        }
        scope.finish().await;
        let command = format!("rm -rf {}", crate::scp::shell_quote(&remote_dir));
        crate::scp::exec::ssh_exec(handle, &command).await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires local MinIO fixture from tests/quicklook/compose.yml"]
async fn quicklook_real_s3_adapter() {
    use s3::{bucket_ops::BucketConfiguration, creds::Credentials, Bucket, Region};
    let credentials =
        Credentials::new(Some("minioadmin"), Some("minioadmin"), None, None, None).unwrap();
    let region = Region::Custom {
        region: "us-east-1".into(),
        endpoint: "http://127.0.0.1:24900".into(),
    };
    let name = format!("quicklook-{}", uuid::Uuid::new_v4());
    let bucket =
        Bucket::create_with_path_style(&name, region, credentials, BucketConfiguration::default())
            .await
            .unwrap()
            .bucket;
    let parent = tempfile::tempdir().unwrap();
    let staging = StagingRoot::new(parent.path()).unwrap();
    let token = CancellationToken::new();
    let scope = crate::ssh::channel_scope::ChannelScope::new(&token);
    for (key, contents) in [
        ("a/測試.txt", b"preview from S3".as_slice()),
        (
            "b/image.png",
            include_bytes!("../../icons/32x32.png").as_slice(),
        ),
        ("c/empty.txt", b"".as_slice()),
    ] {
        bucket.put_object(key, contents).await.unwrap();
        let file = staging.request(key.rsplit('/').next().unwrap()).unwrap();
        staging::download(
            &Source::S3 {
                bucket: bucket.clone(),
                key: key.into(),
            },
            &file,
            &token,
            &scope,
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&file.complete).unwrap(), contents);
        bucket.delete_object(key).await.unwrap();
    }
    let file = staging.request("missing.txt").unwrap();
    assert!(staging::download(
        &Source::S3 {
            bucket: bucket.clone(),
            key: "missing.txt".into()
        },
        &file,
        &token,
        &scope,
        |_, _| {}
    )
    .await
    .is_err());
    scope.finish().await;
    bucket.delete().await.unwrap();
}
