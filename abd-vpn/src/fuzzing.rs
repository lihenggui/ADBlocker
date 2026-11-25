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

//! Entry point for fuzzing whole UDP datagrams.

use std::hint::black_box;

use crate::datagram;

#[cfg(test)]
#[path = "../../fuzz/replay.rs"]
mod replay;

pub fn datagram(input: &[u8]) {
    black_box(datagram::parse(input));
}

#[cfg(test)]
#[test]
fn datagram_fuzz_seeds_replay_without_panicking() {
    replay::seeds("datagram", datagram);
}
