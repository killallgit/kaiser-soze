use crate::BoxError;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, GeneralSubtree, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, NameConstraints,
};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const CERT_FILE: &str = "ca.pem";
const KEY_FILE: &str = "ca-key.pem";

pub(super) struct Ca {
    issuer: Issuer<'static, KeyPair>,
    leaves: Mutex<HashMap<String, Arc<ServerConfig>>>,
}

impl Ca {
    pub(super) fn load_or_create(dir: &Path, permitted_hosts: &[String]) -> Result<Self, BoxError> {
        let cert_path = dir.join(CERT_FILE);
        let key_path = dir.join(KEY_FILE);
        std::fs::create_dir_all(dir)?;
        let key = if key_path.exists() {
            KeyPair::from_pem(&std::fs::read_to_string(&key_path)?)?
        } else {
            create_key(&key_path)?
        };
        let params = ca_params(permitted_hosts)?;
        // Issued at every start: the key and subject stay the same, so certs already trusted keep verifying,
        // and a cert left by another key cannot outlive a start.
        write_atomic(&cert_path, &params.self_signed(&key)?.pem())?;
        Ok(Self {
            issuer: Issuer::new(params, key),
            leaves: Mutex::default(),
        })
    }

    pub(super) fn server_config_for(&self, host: &str) -> Result<Arc<ServerConfig>, BoxError> {
        if let Some(config) = self.leaves.lock().unwrap().get(host) {
            return Ok(config.clone());
        }
        let config = Arc::new(self.sign_leaf(host)?);
        self.leaves.lock().unwrap().insert(host.to_string(), config.clone());
        Ok(config)
    }

    fn sign_leaf(&self, host: &str) -> Result<ServerConfig, BoxError> {
        let mut params = CertificateParams::new(vec![host.to_string()])?;
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        let key = KeyPair::generate()?;
        let cert = params.signed_by(&key, &self.issuer)?;
        let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let mut config = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(vec![cert.der().clone()], key_der)?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(config)
    }
}

fn ca_params(permitted_hosts: &[String]) -> Result<CertificateParams, BoxError> {
    let mut params = CertificateParams::new(Vec::<String>::new())?;
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, "soze local CA");
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.name_constraints = Some(NameConstraints {
        permitted_subtrees: permitted_hosts
            .iter()
            .map(|host| GeneralSubtree::DnsName(host.clone()))
            .collect(),
        excluded_subtrees: Vec::new(),
    });
    Ok(params)
}

/// A name no other start or thread is writing to, beside `path` so a rename or link stays on one filesystem.
fn pending_path(path: &Path) -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let call = NEXT.fetch_add(1, Ordering::Relaxed);
    path.with_extension(format!("{}.{call}.tmp", std::process::id()))
}

/// Creates the key file complete or not at all. A start that loses a race to create it uses the winner's key.
fn create_key(path: &Path) -> Result<KeyPair, BoxError> {
    let key = KeyPair::generate()?;
    let pending = pending_path(path);
    let linked = write_private(&pending, &key.serialize_pem()).and_then(|()| std::fs::hard_link(&pending, path));
    let _ = std::fs::remove_file(&pending);
    match linked {
        Ok(()) => Ok(key),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(KeyPair::from_pem(&std::fs::read_to_string(path)?)?)
        }
        Err(error) => Err(error.into()),
    }
}

fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let pending = pending_path(path);
    std::fs::write(&pending, contents)?;
    std::fs::rename(&pending, path)
}

fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(contents.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, ServerName};
    use rustls::{ClientConfig, RootCertStore};
    use std::os::unix::fs::PermissionsExt;
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    fn hosts(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    async fn handshake(dir: &Path, ca: &Ca, host: &str) -> std::io::Result<()> {
        let pem = std::fs::read(dir.join(CERT_FILE)).unwrap();
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from_pem_slice(&pem).unwrap()).unwrap();
        let client = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let (near, far) = tokio::io::duplex(16 * 1024);
        let acceptor = TlsAcceptor::from(ca.server_config_for(host).unwrap());
        tokio::spawn(async move { acceptor.accept(far).await.ok() });
        TlsConnector::from(Arc::new(client))
            .connect(ServerName::try_from(host.to_string()).unwrap(), near)
            .await
            .map(|_| ())
    }

    #[test]
    fn reuses_each_hosts_leaf_config() {
        let dir = tempfile::tempdir().unwrap();
        let ca = Ca::load_or_create(dir.path(), &hosts(&["a.test", "b.test"])).unwrap();

        let first = ca.server_config_for("a.test").unwrap();
        let repeated = ca.server_config_for("a.test").unwrap();
        let other = ca.server_config_for("b.test").unwrap();

        assert!(Arc::ptr_eq(&first, &repeated));
        assert!(!Arc::ptr_eq(&first, &other));
    }

    #[test]
    fn keeps_the_private_key_readable_only_by_its_owner() {
        // It writes ca-key.pem with mode 0600
        let dir = tempfile::tempdir().unwrap();

        Ca::load_or_create(dir.path(), &hosts(&["a.test"])).unwrap();

        let mode = std::fs::metadata(dir.path().join(KEY_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[tokio::test]
    async fn keeps_one_key_for_good_and_a_cert_that_always_matches_it() {
        // It never replaces the key, and a cert left by another key is replaced at the next start
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        Ca::load_or_create(dir.path(), &hosts(&["a.test"])).unwrap();
        let key = std::fs::read(dir.path().join(KEY_FILE)).unwrap();
        Ca::load_or_create(other.path(), &hosts(&["a.test"])).unwrap();
        std::fs::copy(other.path().join(CERT_FILE), dir.path().join(CERT_FILE)).unwrap();

        let ca = Ca::load_or_create(dir.path(), &hosts(&["a.test"])).unwrap();

        assert_eq!(std::fs::read(dir.path().join(KEY_FILE)).unwrap(), key);
        assert!(handshake(dir.path(), &ca, "a.test").await.is_ok());
    }

    #[tokio::test]
    async fn first_starts_at_the_same_moment_agree_on_one_key() {
        // It lets the loser of the race use the winner's key, so both sign what the cert on disk trusts
        for _ in 0..20 {
            let dir = tempfile::tempdir().unwrap();
            let names = hosts(&["a.test"]);

            let (first, second) = std::thread::scope(|scope| {
                let first = scope.spawn(|| Ca::load_or_create(dir.path(), &names).unwrap());
                let second = scope.spawn(|| Ca::load_or_create(dir.path(), &names).unwrap());
                (first.join().unwrap(), second.join().unwrap())
            });

            assert!(handshake(dir.path(), &first, "a.test").await.is_ok());
            assert!(handshake(dir.path(), &second, "a.test").await.is_ok());
        }
    }

    #[tokio::test]
    async fn clients_accept_permitted_hosts_and_reject_others() {
        // It signs for any host, but a verifying client refuses one outside the constraints
        let dir = tempfile::tempdir().unwrap();
        let ca = Ca::load_or_create(dir.path(), &hosts(&["allowed.test"])).unwrap();

        let allowed = handshake(dir.path(), &ca, "allowed.test").await;
        let outside = handshake(dir.path(), &ca, "outside.test").await;

        assert!(allowed.is_ok());
        let rejection = outside.unwrap_err();
        let tls_error = rejection.get_ref().and_then(|e| e.downcast_ref::<rustls::Error>());
        assert!(matches!(
            tls_error,
            Some(rustls::Error::InvalidCertificate(rustls::CertificateError::Other(_)))
        ));
    }
}
