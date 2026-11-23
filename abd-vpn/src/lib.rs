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

//! Filtering without root: the apps the user chose are brought here by the
//! system, as a VPN the app sets up, a packet at a time; here they are made
//! into connections and given to the engine (`abd_engine`), as the module
//! gives it those the packet filter delivers.

mod datagram;
mod dns;
pub mod engine;
#[cfg(test)]
mod fake;
pub mod fd;
pub mod jni;
mod platform;
pub mod session;
pub mod stack;
pub mod tunnel;
pub mod udp;
