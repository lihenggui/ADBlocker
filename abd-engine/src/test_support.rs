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

use proptest::prelude::*;
use proptest::test_runner::TestCaseResult;

pub(crate) fn arbitrary_text() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..96).prop_map(|chars| chars.into_iter().collect())
}

pub(crate) fn arbitrary_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..128)
}

pub(crate) fn mutate(
    mut bytes: Vec<u8>,
    mutation: Option<(prop::sample::Index, Option<u8>)>,
) -> Vec<u8> {
    if let Some((index, byte)) = mutation {
        let at = index.index(bytes.len() + 1);
        match byte {
            Some(value) => bytes.insert(at, value),
            None if at < bytes.len() => {
                bytes.remove(at);
            }
            None => (),
        }
    }
    bytes
}

pub(crate) fn assert_removal(input: &[u8], output: &[u8]) -> TestCaseResult {
    prop_assert!(output.len() <= input.len());
    let mut kept = 0;
    for byte in input {
        if output.get(kept) == Some(byte) {
            kept += 1;
        }
    }
    prop_assert_eq!(kept, output.len(), "output is not a subsequence");
    Ok(())
}
