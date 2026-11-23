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

//! The certificate authority the device is made to trust.
//!
//! Two roots, because the two stores are judged differently. `system` goes
//! into the system store, which is the only one most apps trust.
//! Chromium requires Certificate Transparency of chains that end in a
//! system-store root, which ours can never have, and exempts roots the user
//! added: `user` goes into the user store for it. Leaves are signed by the
//! key of `user`, and sent with `cross` (the same name and key as `user`,
//! signed by `system`), so one chain ends in either root.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, SanType,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::{CertifiedKey, SigningKey};

pub struct Authority {
    issuer: Issuer<'static, KeyPair>,
    cross: CertificateDer<'static>,
    leaf_key: KeyPair,
    signing_key: Arc<dyn SigningKey>,
    leaves: Mutex<Leaves>,
}

#[derive(Default)]
struct Leaves {
    keys: HashMap<Arc<str>, (Arc<CertifiedKey>, Instant)>,
    order: VecDeque<Arc<str>>,
}

/// The most certificates kept. Evict one in insertion order when full.
const MAX_LEAVES: usize = 2048;
/// How long a certificate is handed out for, of the [`LEAF_DAYS`] it is
/// valid: a daemon that runs for months must not hand out an expired one.
const LEAF_REUSE: Duration = Duration::from_secs(30 * 24 * 3600);
/// Browsers refuse leaves valid for more than 398 days.
const LEAF_DAYS: i64 = 180;

/// Writes the key of the authority, which whoever holds can pass for any
/// server to this device: readable by root only whatever the umask, and
/// never over a file that is there.
fn write_key(file: &Path, pem: &str) -> Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(file)
        .with_context(|| format!("create {}", file.display()))?;
    f.write_all(pem.as_bytes())?;
    f.sync_all()?;
    Ok(())
}

fn ca_params(cn: &str) -> CertificateParams {
    let mut p = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::OrganizationName, "ADBlocker");
    dn.push(DnType::CommonName, cn);
    p.distinguished_name = dn;
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    p.not_before = time::OffsetDateTime::now_utc() - time::Duration::days(2);
    p.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(3650);
    p
}

impl Authority {
    /// Loads the authority from `dir`, creating it on first use.
    pub fn open(dir: &Path) -> Result<Self> {
        use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        // One at a time: at a first start the daemon and `abd update` both
        // come here, and two authorities written over each other are a key
        // with the certificates of another. Held until this returns.
        let lock = std::fs::File::open(dir)?;
        lock.lock()?;
        let f = |n: &str| dir.join(n);
        if !f("user.key").exists() {
            let system_key = KeyPair::generate()?;
            let user_key = KeyPair::generate()?;
            let system_params = ca_params("ADBlocker system CA");
            let system = system_params.self_signed(&system_key)?;
            let user = ca_params("ADBlocker user CA").self_signed(&user_key)?;
            let cross = ca_params("ADBlocker user CA")
                .signed_by(&user_key, &Issuer::from_params(&system_params, &system_key))?;
            std::fs::write(f("system.pem"), system.pem())?;
            std::fs::write(f("user.pem"), user.pem())?;
            std::fs::write(f("cross.pem"), cross.pem())?;
            // The system key signed its one certificate and is thrown away.
            // The other is written last, and what the rest is read by: an
            // authority is either whole or made anew.
            write_key(&f("user.key"), &user_key.serialize_pem())?;
        }
        let issuer_key = KeyPair::from_pem(&std::fs::read_to_string(f("user.key"))?)?;
        // Only the name and key of the issuer go into a leaf.
        let issuer =
            Issuer::from_ca_cert_pem(&std::fs::read_to_string(f("user.pem"))?, issuer_key)?;
        let cross = CertificateDer::from_pem_file(f("cross.pem")).context("cross.pem")?;
        let leaf_key = KeyPair::generate()?;
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
        let signing_key = rustls::crypto::ring::sign::any_supported_type(&key)?;
        Ok(Self {
            issuer,
            cross,
            leaf_key,
            signing_key,
            leaves: Mutex::default(),
        })
    }

    /// A certificate for `host` (a DNS name or an IP address).
    pub fn leaf(&self, host: &str) -> Result<Arc<CertifiedKey>> {
        if let Some((k, minted)) = self.leaves.lock().unwrap().keys.get(host)
            && minted.elapsed() < LEAF_REUSE
        {
            return Ok(k.clone());
        }
        let mut p = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, host);
        p.distinguished_name = dn;
        p.subject_alt_names = vec![match host.parse() {
            Ok(ip) => SanType::IpAddress(ip),
            Err(_) => SanType::DnsName(host.try_into()?),
        }];
        p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        p.use_authority_key_identifier_extension = true;
        p.not_before = time::OffsetDateTime::now_utc() - time::Duration::days(2);
        p.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(LEAF_DAYS);
        let cert = p.signed_by(&self.leaf_key, &self.issuer)?;
        let ck = Arc::new(CertifiedKey::new(
            vec![cert.der().clone(), self.cross.clone()],
            self.signing_key.clone(),
        ));
        let mut leaves = self.leaves.lock().unwrap();
        if let Some((old, minted)) = leaves.keys.get_mut(host) {
            // Another connection may have minted this certificate while
            // the lock was released.
            if minted.elapsed() < LEAF_REUSE {
                return Ok(old.clone());
            }
            *old = ck.clone();
            *minted = Instant::now();
        } else {
            if leaves.keys.len() == MAX_LEAVES {
                let oldest = leaves.order.pop_front().expect("full leaf cache");
                leaves.keys.remove(&oldest);
            }
            let host: Arc<str> = Arc::from(host);
            leaves.order.push_back(host.clone());
            leaves.keys.insert(host, (ck.clone(), Instant::now()));
        }
        Ok(ck)
    }
}

#[cfg(test)]
mod tests {
    use rustls::RootCertStore;
    use rustls::client::WebPkiServerVerifier;
    use rustls::client::danger::ServerCertVerifier;
    use rustls::pki_types::{ServerName, UnixTime};

    use super::*;

    #[test]
    fn a_full_leaf_cache_keeps_its_other_certificates_and_shares_the_signing_key() {
        let dir = tempfile::tempdir().unwrap();
        let ca = Authority::open(dir.path()).unwrap();
        let first = ca.leaf("first.example").unwrap();
        let kept = ca.leaf("kept.example").unwrap();
        for n in 2..MAX_LEAVES {
            ca.leaf(&format!("host{n}.example")).unwrap();
        }
        let newest = ca.leaf("new.example").unwrap();
        assert!(Arc::ptr_eq(&newest.key, &first.key));
        assert!(Arc::ptr_eq(&kept, &ca.leaf("kept.example").unwrap()));
        assert!(!Arc::ptr_eq(&first, &ca.leaf("first.example").unwrap()));
        assert_eq!(ca.leaves.lock().unwrap().keys.len(), MAX_LEAVES);
    }

    #[test]
    #[ignore = "a measurement, run by hand"]
    fn measures_minting_and_turnover_of_leaf_certificates() {
        let dir = tempfile::tempdir().unwrap();
        let ca = Authority::open(dir.path()).unwrap();
        let started = Instant::now();
        for n in 0..MAX_LEAVES {
            ca.leaf(&format!("host{n}.example")).unwrap();
        }
        println!(
            "cold leaf mint: {:?}/host",
            started.elapsed() / MAX_LEAVES as u32
        );
        let started = Instant::now();
        for n in 0..256 {
            ca.leaf(&format!("new{n}.example")).unwrap();
            ca.leaf(&format!("host{}.example", MAX_LEAVES - 1 - n))
                .unwrap();
        }
        println!("leaf turnover: {:?}/pair", started.elapsed() / 256);
    }

    /// Whether a client that trusts only the root in `file` accepts the
    /// chain sent for `host`.
    fn accepted_with(dir: &Path, file: &str, host: &str, chain: &CertifiedKey) -> bool {
        let mut roots = RootCertStore::empty();
        let pem = std::fs::read(dir.join(file)).unwrap();
        for cert in CertificateDer::pem_slice_iter(&pem) {
            roots.add(cert.unwrap()).unwrap();
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider)
            .build()
            .unwrap();
        verifier
            .verify_server_cert(
                &chain.cert[0],
                &chain.cert[1..],
                &ServerName::try_from(host.to_owned()).unwrap(),
                &[],
                UnixTime::now(),
            )
            .is_ok()
    }

    #[test]
    fn one_chain_ends_in_either_root() {
        let dir = tempfile::tempdir().unwrap();
        let ca = Authority::open(dir.path()).unwrap();
        let chain = ca.leaf("www.example.com").unwrap();
        // An app, which trusts the system store only.
        assert!(accepted_with(
            dir.path(),
            "system.pem",
            "www.example.com",
            &chain
        ));
        // Chromium, which is given the root of the user store.
        assert!(accepted_with(
            dir.path(),
            "user.pem",
            "www.example.com",
            &chain
        ));
        assert!(!accepted_with(
            dir.path(),
            "user.pem",
            "other.example.com",
            &chain
        ));
    }

    #[test]
    fn the_authority_is_the_same_when_opened_again() {
        let dir = tempfile::tempdir().unwrap();
        let system = {
            Authority::open(dir.path()).unwrap();
            std::fs::read(dir.path().join("system.pem")).unwrap()
        };
        let again = Authority::open(dir.path()).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("system.pem")).unwrap(),
            system
        );
        // What the second signs is accepted under the roots of the first.
        let chain = again.leaf("a.example").unwrap();
        assert!(accepted_with(dir.path(), "system.pem", "a.example", &chain));
    }

    #[test]
    fn signs_for_an_address_as_well_as_a_name() {
        let dir = tempfile::tempdir().unwrap();
        let ca = Authority::open(dir.path()).unwrap();
        let chain = ca.leaf("192.0.2.7").unwrap();
        assert!(accepted_with(dir.path(), "user.pem", "192.0.2.7", &chain));
    }

    #[test]
    fn a_certificate_is_minted_once_for_a_host() {
        let dir = tempfile::tempdir().unwrap();
        let ca = Authority::open(dir.path()).unwrap();
        let (a, b) = (ca.leaf("a.example").unwrap(), ca.leaf("a.example").unwrap());
        assert!(Arc::ptr_eq(&a, &b));
    }
}

#[cfg(test)]
mod key_tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn the_key_is_readable_by_its_owner_only_whatever_the_umask() {
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca");
        Authority::open(&ca).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&ca.join("user.key")), 0o600);
        assert_eq!(mode(&ca), 0o700);
        // Nothing but that one key is kept: the system root's is gone.
        let names: Vec<_> = std::fs::read_dir(&ca)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 4);
    }
}
