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
use std::sync::{Arc, Mutex};

const CERT_FILE: &str = "ca.pem";
const KEY_FILE: &str = "ca-key.pem";
const HOSTS_FILE: &str = "ca-hosts.txt";

pub(super) struct Ca {
    issuer: Issuer<'static, KeyPair>,
    leaves: Mutex<HashMap<String, Arc<ServerConfig>>>,
}

impl Ca {
    pub(super) fn load_or_create(dir: &Path, permitted_hosts: &[String]) -> Result<Self, BoxError> {
        let cert_path = dir.join(CERT_FILE);
        let key_path = dir.join(KEY_FILE);
        let hosts_path = dir.join(HOSTS_FILE);
        std::fs::create_dir_all(dir)?;
        let key = if key_path.exists() {
            KeyPair::from_pem(&std::fs::read_to_string(&key_path)?)?
        } else {
            let key = KeyPair::generate()?;
            write_private(&key_path, &key.serialize_pem())?;
            key
        };
        let params = ca_params(permitted_hosts)?;
        let hosts = permitted_hosts.join("\n");
        let stored_hosts = std::fs::read_to_string(&hosts_path).ok();
        if !cert_path.exists() || stored_hosts.as_deref() != Some(hosts.as_str()) {
            std::fs::write(&cert_path, params.self_signed(&key)?.pem())?;
            std::fs::write(&hosts_path, hosts)?;
        }
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

    #[test]
    fn reuses_the_key_and_reissues_the_cert_only_when_hosts_change() {
        // It keeps one key for good, and a new cert exactly when the permitted hosts differ
        let dir = tempfile::tempdir().unwrap();
        let read = |file: &str| std::fs::read(dir.path().join(file)).unwrap();
        Ca::load_or_create(dir.path(), &hosts(&["a.test"])).unwrap();
        let (key, cert) = (read(KEY_FILE), read(CERT_FILE));

        Ca::load_or_create(dir.path(), &hosts(&["a.test"])).unwrap();
        let unchanged = read(CERT_FILE) == cert;
        Ca::load_or_create(dir.path(), &hosts(&["a.test", "b.test"])).unwrap();

        assert!(unchanged);
        assert_eq!(read(KEY_FILE), key);
        assert_ne!(read(CERT_FILE), cert);
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
