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

use std::collections::BTreeSet;

use anyhow::Result;

use crate::{dns, rules};

/// The rules that bring the active apps' connections and lookups here,
/// changed together for the same apps.
pub(super) struct CaptureRules {
    tcp: rules::Rules,
    dns: dns::Dns,
}

impl CaptureRules {
    pub(super) fn new(tcp: rules::Rules, dns: dns::Dns) -> Self {
        Self { tcp, dns }
    }

    /// Captures the apps of `uids`. Lookups follow what connections do:
    /// none are brought for apps whose connections are not. A failure of
    /// lookup capture is its own, and leaves connections filtered.
    pub(super) fn set(
        &mut self,
        uids: &BTreeSet<u32>,
        families: rules::Families,
        owners: rules::Owners,
    ) -> Result<()> {
        let tcp = self.tcp.set(uids, families, owners);
        let lookups = if tcp.is_ok() { uids } else { &BTreeSet::new() };
        self.dns.set(lookups, families);
        tcp
    }

    /// Withdraws the lookup rules first: they need no lock, and the TCP
    /// rules may wait for one.
    pub(super) fn remove(&mut self) -> Result<()> {
        let withdrawn = self.dns.withdraw();
        let tcp = self.tcp.remove();
        let lookups = self.dns.remove();
        withdrawn.and(tcp).and(lookups)
    }
}
