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

//! Reads the public trust material of a managed profile.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

use super::{CA_LIMIT, store_name, system_store};
use crate::paths::Paths;

pub const PROFILE_STORES: &str = "/data/misc/user";

fn cert_at(path: &Path) -> Result<CertificateDer<'static>> {
    let bytes = abd_engine::files::read_bytes(path, CA_LIMIT)?;
    if bytes.starts_with(b"-----BEGIN") {
        Ok(CertificateDer::from_pem_slice(&bytes)?)
    } else {
        let (rest, _) = x509_parser::parse_x509_certificate(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid CA certificate"))?;
        ensure!(rest.is_empty(), "invalid CA certificate");
        Ok(CertificateDer::from(bytes))
    }
}

/// Android owns these stores. A managed profile can use full filtering
/// only after the user or administrator installs our public user CA in
/// that profile, and verification excludes roots disabled there.
pub fn profile_roots(paths: &Paths, profile: u32) -> Result<RootCertStore> {
    let user = Path::new(PROFILE_STORES).join(profile.to_string());
    let pem = paths.ca().join("user.pem");
    let name = store_name(&abd_engine::files::read_bytes(&pem, CA_LIMIT)?)?;
    let ours = cert_at(&pem)?;
    let system_ca = cert_at(&paths.ca().join("system.pem"))?;
    profile_store(&user, system_store(), &name, &ours, &system_ca)
}

fn profile_store(
    user: &Path,
    system: &Path,
    name: &str,
    ours: &CertificateDer<'_>,
    system_ca: &CertificateDer<'_>,
) -> Result<RootCertStore> {
    let hash = name
        .strip_suffix(".0")
        .context("invalid certificate store name")?;
    let added = user.join("cacerts-added");
    let mut installed = false;
    // Android numbers collisions consecutively. A certificate behind a
    // gap is invisible to clients looking up the subject hash.
    for index in 0..32 {
        let path = added.join(format!("{hash}.{index}"));
        match abd_engine::files::read_bytes(&path, CA_LIMIT) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Ok(bytes) if bytes.is_empty() => continue, // Android's tombstone.
            Ok(_) if cert_at(&path).is_ok_and(|cert| &cert == ours) => {
                installed = true;
                break;
            }
            Ok(_) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    ensure!(installed, "the managed profile has no ADBlocker CA");

    let removed = user.join("cacerts-removed");
    let disabled: BTreeSet<_> = match std::fs::read_dir(removed) {
        Ok(entries) => entries
            .map(|entry| entry.map(|e| e.file_name()))
            .collect::<std::io::Result<_>>()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
        Err(e) => return Err(e.into()),
    };
    let mut roots = RootCertStore::empty();
    for entry in std::fs::read_dir(system)? {
        let entry = entry?;
        // Android's SystemCertificateSource excludes the marker by name.
        // Skipping a bad or stale marker too is deliberately conservative.
        if !disabled.contains(&entry.file_name())
            && let Ok(cert) = cert_at(&entry.path())
            && &cert != ours
            && &cert != system_ca
        {
            roots.add(cert).ok();
        }
    }
    ensure!(
        !roots.is_empty(),
        "the managed profile has no usable system roots"
    );
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_store_requires_the_exact_local_root_and_honors_disabled_system_roots() {
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca");
        abd_engine::ca::Authority::open(&ca).unwrap();
        let ours = cert_at(&ca.join("user.pem")).unwrap();
        let system_ca = cert_at(&ca.join("system.pem")).unwrap();
        let name = store_name(&std::fs::read(ca.join("user.pem")).unwrap()).unwrap();
        let system = dir.path().join("system");
        std::fs::create_dir(&system).unwrap();
        std::fs::copy(ca.join("system.pem"), system.join("local.0")).unwrap();
        std::fs::write(
            system.join("public.0"),
            include_bytes!("../../testdata/isrg-root-x1.pem"),
        )
        .unwrap();

        let user = dir.path().join("10");
        let added = user.join("cacerts-added");
        std::fs::create_dir_all(&added).unwrap();
        let removed = user.join("cacerts-removed");
        std::fs::create_dir(&removed).unwrap();
        let local = added.join(&name);
        let roots = || profile_store(&user, &system, &name, &ours, &system_ca);
        assert!(roots().is_err());
        std::fs::write(&local, &system_ca).unwrap();
        assert!(roots().is_err());
        std::fs::write(&local, &ours).unwrap(); // Android writes the certificate as DER.
        assert_eq!(roots().unwrap().len(), 1);
        std::fs::write(removed.join("public.0"), b"marker").unwrap();
        assert!(roots().is_err());
        std::fs::remove_file(removed.join("public.0")).unwrap();
        std::fs::write(&local, []).unwrap(); // Android leaves a tombstone on removal.
        assert!(roots().is_err());
        let collision = added.join(format!("{}.1", name.strip_suffix(".0").unwrap()));
        std::fs::write(&collision, &ours).unwrap();
        assert_eq!(roots().unwrap().len(), 1);
        std::fs::remove_file(&local).unwrap();
        assert!(roots().is_err());
    }
}
