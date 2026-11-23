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

//! The device our table's routes go through. Nothing is ever read from it
//! or sent on it: the routes are local, and a lookup routed by them is
//! delivered on the loopback interface, where TPROXY hands it to us.
//!
//! It is what keeps a dead daemon from taking the apps' lookups with it.
//! The device belongs to the daemon's process: when the process ends,
//! however it ends, the device is gone, its routes with it, and our rules
//! lead to an empty table, which the kernel passes over.

use std::os::fd::OwnedFd;

use anyhow::Context as _;

use super::netlink::{self, Netlink};
use super::{DEVICE, SOURCE_V4, SOURCE_V6, sys};

pub(super) struct Anchor {
    _device: OwnedFd,
    /// Whether IPv6 lookups have a route too.
    pub(super) v6: bool,
}

impl Anchor {
    pub(super) fn open(netlink: &mut Netlink) -> anyhow::Result<Self> {
        let device = sys::open_tun(DEVICE).context("creating the anchor device")?;
        let index = netlink.index().context("finding the anchor device")?;
        netlink
            .change(netlink::link_up(index))
            .context("bringing the anchor device up")?;
        netlink
            .change(netlink::route(index, SOURCE_V4.into()))
            .context("routing IPv4 lookups")?;
        // Without an address of its own on the device, the kernel takes
        // no IPv6 source for the route.
        let v6 = netlink
            .change(netlink::address_v6(index, SOURCE_V6.into()))
            .and_then(|()| netlink.change(netlink::route(index, SOURCE_V6.into())));
        if let Err(error) = &v6 {
            eprintln!("dns: no IPv6 route ({error}); lookups over IPv6 are not filtered");
        }
        Ok(Self {
            _device: device,
            v6: v6.is_ok(),
        })
    }
}
