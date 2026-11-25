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

//! Shared seed replay for the owning crates' unit tests.

use std::{fs, path::Path};

pub fn seeds(target: &str, run: fn(&[u8])) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fuzz/seeds")
        .join(target);
    let mut paths: Vec<_> = fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
                .path()
        })
        .collect();
    assert!(!paths.is_empty(), "no seeds in {}", dir.display());
    paths.sort_unstable();
    for path in paths {
        let input = fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        eprintln!("replaying {target}/{}", path.file_name().unwrap().display());
        run(&input);
    }
}
