// Copyright (C) 2026 Mercury Li
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License version 3 as
// published by the Free Software Foundation.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! The TLS configurations of both sides of a decrypted connection, made
//! once and shared by every connection that can use them.
//!
//! What a client and a server remember of each other, to resume a session
//! without a full handshake, is kept by the configuration. One made for
//! each connection remembers nothing: every connection of a browser, which
//! opens many to one site, would then cost two full handshakes, in time on
//! the network and in work for the processor.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use rustls::client::Resumption;
use rustls::crypto::CryptoProvider;
use rustls::server::{ClientHello, ResolvesServerCert, ServerSessionMemoryCache};
use rustls::sign::CertifiedKey;
use rustls::{ClientConfig, RootCertStore, ServerConfig};

use crate::ca::Authority;
use crate::clienthello::Hello;

/// The sessions remembered on each side. A session is a few hundred bytes.
const SESSIONS: usize = 1024;
const MAX_CERTIFICATE_CHECKS: usize = 1024;

/// What a client configuration is made for: the protocols offered, and
/// whether anything older than TLS 1.3 is.
type ClientKey = (Vec<Vec<u8>>, bool);

pub struct Tls {
    provider: Arc<CryptoProvider>,
    /// What servers are verified against.
    roots: Arc<RootCertStore>,
    ca: Arc<Authority>,
    /// By what the app offered. Of the protocols only the two that can be
    /// read here are ever offered on, so there are a handful at most.
    clients: Mutex<HashMap<ClientKey, Arc<ClientConfig>>>,
    /// By the protocol the server chose: none, HTTP/1.1 or HTTP/2.
    servers: Mutex<HashMap<Option<Vec<u8>>, Arc<ServerConfig>>>,
    /// Only a fixed-size certificate fingerprint is retained with each result.
    ev: Mutex<HashMap<String, (ring::digest::Digest, bool)>>,
}

/// Answers each client with a certificate for the name it asked for.
struct ByName(Arc<Authority>);

impl std::fmt::Debug for ByName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Nothing of the authority: it holds a key.
        f.write_str("ByName")
    }
}

impl ResolvesServerCert for ByName {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.0.leaf(hello.server_name()?).ok()
    }
}

/// The policy every Extended Validation certificate names
/// (2.23.140.1.1, of the CA/Browser Forum), as it is encoded.
const EV_POLICY: [u8; 5] = [0x67, 0x81, 0x0c, 0x01, 0x01];

/// Whether a server's certificate is an Extended Validation one: issued
/// after its owner's legal identity was checked, as those of banks and
/// payment services mostly are. Connections to such servers are not
/// decrypted.
pub fn extended_validation(certificate: &[u8]) -> bool {
    use x509_parser::extensions::ParsedExtension;
    let Ok((_, certificate)) = x509_parser::parse_x509_certificate(certificate) else {
        return false;
    };
    certificate.extensions().iter().any(|extension| {
        matches!(
            extension.parsed_extension(),
            ParsedExtension::CertificatePolicies(policies)
                if policies.iter().any(|p| p.policy_id.as_bytes() == EV_POLICY)
        )
    })
}

/// The protocols that can be read here.
fn readable(protocol: &[u8]) -> bool {
    protocol == b"h2" || protocol == b"http/1.1"
}

impl Tls {
    pub fn new(ca: Authority, roots: RootCertStore) -> Self {
        Self::with_authority(Arc::new(ca), roots)
    }

    fn with_authority(ca: Arc<Authority>, roots: RootCertStore) -> Self {
        Self {
            provider: Arc::new(rustls::crypto::ring::default_provider()),
            roots: Arc::new(roots),
            ca,
            clients: Mutex::default(),
            servers: Mutex::default(),
            ev: Mutex::default(),
        }
    }

    pub(crate) fn extended_validation(&self, host: &str, certificate: &[u8]) -> bool {
        let fingerprint = ring::digest::digest(&ring::digest::SHA256, certificate);
        if let Some((previous, result)) = self.ev.lock().unwrap().get(host)
            && previous.as_ref() == fingerprint.as_ref()
        {
            return *result;
        }
        let result = extended_validation(certificate);
        let mut ev = self.ev.lock().unwrap();
        if let Some(cached) = ev.get_mut(host) {
            *cached = (fingerprint, result);
        } else {
            if ev.len() == MAX_CERTIFICATE_CHECKS {
                let victim = ev.keys().next().expect("full certificate cache").clone();
                ev.remove(&victim);
            }
            ev.insert(host.to_owned(), (fingerprint, result));
        }
        result
    }

    /// A separately verified Android user must also have separate TLS sessions.
    pub fn with_roots(&self, roots: RootCertStore) -> Self {
        Self::with_authority(self.ca.clone(), roots)
    }

    pub fn has_roots(&self, roots: &RootCertStore) -> bool {
        self.roots.roots == roots.roots
    }

    /// For connecting to the server an app was connecting to with `hello`.
    pub fn client(&self, hello: &Hello) -> Result<Arc<ClientConfig>> {
        // Of what the app offers, what can be read here, in its order.
        let alpn: Vec<Vec<u8>> = hello.alpn.iter().filter(|p| readable(p)).cloned().collect();
        let key = (alpn, hello.tls13_only);
        let mut clients = self.clients.lock().unwrap();
        if let Some(config) = clients.get(&key) {
            return Ok(config.clone());
        }
        // An app that would speak nothing older than TLS 1.3 is not made
        // to, on the half of its connection it cannot see.
        let versions: &[&rustls::SupportedProtocolVersion] = if key.1 {
            &[&rustls::version::TLS13]
        } else {
            rustls::DEFAULT_VERSIONS
        };
        let mut config = ClientConfig::builder_with_provider(self.provider.clone())
            .with_protocol_versions(versions)?
            .with_root_certificates(self.roots.clone())
            .with_no_client_auth();
        config.alpn_protocols.clone_from(&key.0);
        config.resumption = Resumption::in_memory_sessions(SESSIONS);
        let config = Arc::new(config);
        clients.insert(key, config.clone());
        Ok(config)
    }

    /// For answering an app, offering it the protocol the server chose.
    pub fn server(&self, alpn: Option<&[u8]>) -> Result<Arc<ServerConfig>> {
        let key = alpn.map(<[u8]>::to_vec);
        let mut servers = self.servers.lock().unwrap();
        if let Some(config) = servers.get(&key) {
            return Ok(config.clone());
        }
        let mut config = ServerConfig::builder_with_provider(self.provider.clone())
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(ByName(self.ca.clone())));
        config.alpn_protocols = key.iter().cloned().collect();
        config.session_storage = ServerSessionMemoryCache::new(SESSIONS);
        let config = Arc::new(config);
        servers.insert(key, config.clone());
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tls() -> (Tls, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let ca = Authority::open(dir.path()).unwrap();
        (Tls::new(ca, RootCertStore::empty()), dir)
    }

    fn hello(alpn: &[&str], tls13_only: bool) -> Hello {
        Hello {
            alpn: alpn.iter().map(|p| p.as_bytes().to_vec()).collect(),
            tls13_only,
            ..Hello::default()
        }
    }

    #[test]
    fn connections_that_offer_the_same_share_one_configuration() {
        let (tls, _dir) = tls();
        let a = tls.client(&hello(&["h2", "http/1.1"], false)).unwrap();
        let b = tls.client(&hello(&["h2", "http/1.1"], false)).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        let c = tls.client(&hello(&["http/1.1"], false)).unwrap();
        assert!(!Arc::ptr_eq(&a, &c));
        let d = tls.client(&hello(&["h2", "http/1.1"], true)).unwrap();
        assert!(!Arc::ptr_eq(&a, &d));
        assert!(Arc::ptr_eq(
            &tls.server(Some(b"h2")).unwrap(),
            &tls.server(Some(b"h2")).unwrap()
        ));
    }

    /// A certificate naming the given policy, as encoded.
    pub fn certificate_with_policy(policy: &[u8]) -> Vec<u8> {
        use rcgen::{CertificateParams, CustomExtension, KeyPair};
        let mut params = CertificateParams::new(vec!["bank.example".to_owned()]).unwrap();
        // certificatePolicies: SEQUENCE { SEQUENCE { OID } }
        let mut content = vec![0x30, policy.len() as u8 + 4, 0x30, policy.len() as u8 + 2];
        content.extend_from_slice(&[0x06, policy.len() as u8]);
        content.extend_from_slice(policy);
        params
            .custom_extensions
            .push(CustomExtension::from_oid_content(&[2, 5, 29, 32], content));
        let key = KeyPair::generate().unwrap();
        params.self_signed(&key).unwrap().der().to_vec()
    }

    #[test]
    #[ignore = "a measurement, run by hand"]
    fn measures_extended_validation_certificate_checks() {
        let cert = certificate_with_policy(&EV_POLICY);
        let start = std::time::Instant::now();
        for _ in 0..50_000 {
            assert!(std::hint::black_box(extended_validation(
                std::hint::black_box(&cert)
            )));
        }
        println!("EV parse: {:?}/certificate", start.elapsed() / 50_000);
        let start = std::time::Instant::now();
        for _ in 0..50_000 {
            std::hint::black_box(ring::digest::digest(
                &ring::digest::SHA256,
                std::hint::black_box(&cert),
            ));
        }
        println!(
            "certificate digest: {:?}/certificate",
            start.elapsed() / 50_000
        );
        let dir = tempfile::tempdir().unwrap();
        let tls = Tls::new(Authority::open(dir.path()).unwrap(), RootCertStore::empty());
        let start = std::time::Instant::now();
        for _ in 0..50_000 {
            assert!(std::hint::black_box(
                tls.extended_validation("example.com", &cert)
            ));
        }
        println!("EV cache hit: {:?}/certificate", start.elapsed() / 50_000);
    }

    #[test]
    fn a_certificate_change_can_add_or_remove_extended_validation() {
        let dir = tempfile::tempdir().unwrap();
        let tls = Tls::new(Authority::open(dir.path()).unwrap(), RootCertStore::empty());
        let ev = certificate_with_policy(&EV_POLICY);
        let dv = certificate_with_policy(&[0x67, 0x81, 0x0c, 0x01, 0x02]);
        assert!(!tls.extended_validation("example.com", &dv));
        assert!(tls.extended_validation("example.com", &ev));
        assert!(!tls.extended_validation("example.com", &dv));
    }

    #[test]
    fn tells_an_extended_validation_certificate_by_its_policy() {
        assert!(extended_validation(&certificate_with_policy(&EV_POLICY)));
        // Domain validated: 2.23.140.1.2.1.
        let dv = [0x67, 0x81, 0x0c, 0x01, 0x02, 0x01];
        assert!(!extended_validation(&certificate_with_policy(&dv)));
        assert!(!extended_validation(b"not a certificate"));
        // With no policies at all.
        let dir = tempfile::tempdir().unwrap();
        let ca = Authority::open(dir.path()).unwrap();
        assert!(!extended_validation(&ca.leaf("a.example").unwrap().cert[0]));
    }

    #[test]
    fn only_protocols_that_can_be_read_are_offered_on() {
        let (tls, _dir) = tls();
        let config = tls
            .client(&hello(&["h3", "h2", "spdy/3", "http/1.1"], false))
            .unwrap();
        assert_eq!(
            config.alpn_protocols,
            [b"h2".to_vec(), b"http/1.1".to_vec()]
        );
        // And what an app makes up does not add configurations without end.
        for n in 0..100 {
            tls.client(&hello(&[&format!("made-up-{n}")], false))
                .unwrap();
        }
        assert!(tls.clients.lock().unwrap().len() <= 3);
    }
}
