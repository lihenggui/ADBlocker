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

use anyhow::{Context as _, Result, bail};
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
    // Those the user turned off in the settings are no more ours to
    // believe than the app's.
    let removed: std::collections::HashSet<_> = std::fs::read_dir(REMOVED_ROOTS)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    let mut store = rustls::RootCertStore::empty();
    for dir in [
        "/apex/com.android.conscrypt/cacerts",
        "/system/etc/security/cacerts",
        "/etc/ssl/certs",
    ] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in entries.flatten() {
            if removed.contains(&e.file_name()) {
                continue;
            }
            let Ok(pem) = std::fs::read(e.path()) else {
                continue;
            };
            for cert in CertificateDer::pem_slice_iter(&pem).flatten() {
                // Where the store is the copy mounted by trust.rs.
                if cert != ours {
                    store.add(cert).ok();
                }
            }
        }
        if !store.is_empty() {
            return Ok(store);
        }
    }
    bail!("no system roots found")
}
