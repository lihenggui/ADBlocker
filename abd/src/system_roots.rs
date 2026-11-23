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

use std::path::Path;

use anyhow::{Context as _, Result, ensure};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

use crate::paths::Paths;

/// Where the system keeps the system roots the first user turned off,
/// each under the name it has in the store.
const REMOVED_ROOTS: &str = "/data/misc/user/0/cacerts-removed";

/// The roots servers are verified against: the system's, without ours.
pub fn read(paths: &Paths) -> Result<rustls::RootCertStore> {
    let ours = std::fs::read(paths.ca().join("system.pem"))?;
    let ours = CertificateDer::from_pem_slice(&ours).context("system.pem")?;
    // The store the apps are given ours in (`trust.rs`), and no other.
    read_in(
        crate::trust::system_store(),
        Path::new(REMOVED_ROOTS),
        &ours,
    )
}

fn read_in(dir: &Path, removed: &Path, ours: &CertificateDer<'_>) -> Result<rustls::RootCertStore> {
    // Those the user turned off in the settings are no more ours to
    // believe than the app's.
    let removed: std::collections::HashSet<_> = std::fs::read_dir(removed)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    let mut store = rustls::RootCertStore::empty();
    // A root that cannot be read or taken is left out: a server that has
    // only it to chain to is not verified, and is handed on undecrypted.
    let mut left_out = 0;
    for e in std::fs::read_dir(dir).with_context(|| dir.display().to_string())? {
        let Ok(e) = e else {
            left_out += 1;
            continue;
        };
        if removed.contains(&e.file_name()) {
            continue;
        }
        let Ok(pem) = std::fs::read(e.path()) else {
            left_out += 1;
            continue;
        };
        for cert in CertificateDer::pem_slice_iter(&pem) {
            let Ok(cert) = cert else {
                left_out += 1;
                continue;
            };
            // Where the store is the copy mounted by trust.rs.
            if cert != *ours && store.add(cert).is_err() {
                left_out += 1;
            }
        }
    }
    if left_out > 0 {
        eprintln!("system roots: {left_out} could not be read and are left out");
    }
    ensure!(!store.is_empty(), "no system roots found");
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &[u8] = include_bytes!("../testdata/isrg-root-x1.pem");

    #[test]
    fn the_system_roots_are_the_stores_without_ours_or_those_turned_off() {
        let dir = tempfile::tempdir().unwrap();
        let (store, removed) = (dir.path().join("store"), dir.path().join("removed"));
        std::fs::create_dir(&store).unwrap();
        std::fs::create_dir(&removed).unwrap();
        let ca = dir.path().join("ca");
        abd_engine::ca::Authority::open(&ca).unwrap();
        let pem = std::fs::read(ca.join("system.pem")).unwrap();
        let ours = CertificateDer::from_pem_slice(&pem).unwrap();
        std::fs::write(store.join("494499dc.0"), &pem).unwrap();
        std::fs::write(store.join("6187b673.0"), ROOT).unwrap();
        assert_eq!(read_in(&store, &removed, &ours).unwrap().len(), 1);

        // A root the user turned off is no root, and no root is a failure.
        std::fs::write(removed.join("6187b673.0"), ROOT).unwrap();
        assert!(read_in(&store, &removed, &ours).is_err());
    }
}
