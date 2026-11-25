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

#![no_main]

use std::hint::black_box;

use libfuzzer_sys::fuzz_target;

// Compile the same parser as the daemon without bringing in the daemon binary.
#[allow(
    dead_code,
    reason = "The target exercises parsing without using the extracted metadata."
)]
#[path = "../../abd/src/abx.rs"]
mod abx;

fuzz_target!(|input: &[u8]| {
    black_box(abx::elements(input));
    black_box(abx::package_elements(input));
});
