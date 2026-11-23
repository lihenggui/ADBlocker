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

//! The ABX v0 tags used by Android's user and package metadata. The wire format is
//! defined by AOSP's `com.android.modules.utils.BinaryXmlSerializer`.

use std::collections::BTreeMap;

pub const MAGIC: &[u8] = b"ABX\0";

pub struct Element<'a> {
    pub name: &'a [u8],
    pub depth: usize,
    pub attrs: BTreeMap<&'a [u8], String>,
}

struct Reader<'a> {
    data: &'a [u8],
    strings: Vec<&'a [u8]>,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (head, tail) = self.data.split_at_checked(count)?;
        self.data = tail;
        Some(head)
    }

    fn length(&mut self) -> Option<usize> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?).into())
    }

    fn string(&mut self) -> Option<&'a [u8]> {
        let len = self.length()?;
        self.take(len)
    }

    fn interned(&mut self) -> Option<&'a [u8]> {
        let index = self.length()?;
        if index != usize::from(u16::MAX) {
            return self.strings.get(index).copied();
        }
        let value = self.string()?;
        if self.strings.len() < usize::from(u16::MAX) {
            self.strings.push(value);
        }
        Some(value)
    }

    fn value(&mut self, kind: u8, retain: bool, limit: usize) -> Option<Option<String>> {
        let value = match kind {
            1 => None,
            // Names and user-type attributes are ASCII. Android's modified
            // UTF-8 in unrelated values is immaterial to profile resolution.
            2 | 3 => {
                let bytes = if kind == 2 {
                    self.string()?
                } else {
                    self.interned()?
                };
                if retain {
                    if bytes.len() > limit {
                        return None;
                    }
                    std::str::from_utf8(bytes).ok().map(str::to_owned)
                } else {
                    None
                }
            }
            4 | 5 => {
                self.string()?;
                None
            }
            6 | 7 => {
                let n = i32::from_be_bytes(self.take(4)?.try_into().ok()?);
                retain.then(|| n.to_string())
            }
            8 | 9 => {
                let n = i64::from_be_bytes(self.take(8)?.try_into().ok()?);
                retain.then(|| n.to_string())
            }
            10 | 11 => {
                self.take(if kind == 10 { 4 } else { 8 })?;
                None
            }
            12 => retain.then(|| "true".to_owned()),
            13 => retain.then(|| "false".to_owned()),
            _ => return None,
        };
        Some(value)
    }
}

/// Read the root and its immediate children from a complete ABX document.
/// Unknown documents or truncated writes are rejected as a whole.
pub fn elements(bytes: &[u8]) -> Option<Vec<Element<'_>>> {
    read_elements(bytes, Metadata::Users)
}

/// Only the fields Android uses to map apps to named shared UIDs. The
/// package registry also contains signatures and other large, interned
/// attributes that never need to become owned strings.
pub fn package_elements(bytes: &[u8]) -> Option<Vec<Element<'_>>> {
    read_elements(bytes, Metadata::Packages)
}

#[derive(Clone, Copy)]
enum Metadata {
    Users,
    Packages,
}

impl Metadata {
    fn collects(self, name: &[u8], depth: usize) -> bool {
        depth == 0
            || (depth == 1
                && (matches!(self, Self::Users) || matches!(name, b"package" | b"shared-user")))
    }

    fn retains(self, name: &[u8]) -> bool {
        matches!(self, Self::Users) || matches!(name, b"name" | b"userId" | b"sharedUserId")
    }
}

fn read_elements(bytes: &[u8], metadata: Metadata) -> Option<Vec<Element<'_>>> {
    // Package names fit in 255 bytes. User-record values were already bounded by
    // the much smaller input file, including its modified UTF-8 strings.
    let value_limit = match metadata {
        Metadata::Packages => 255,
        Metadata::Users => usize::MAX,
    };
    let mut reader = Reader {
        data: bytes.strip_prefix(MAGIC)?.strip_prefix(&[0x10])?,
        strings: Vec::new(),
    };
    let mut open = Vec::new();
    let mut output = Vec::new();
    let mut attr_element = None;
    loop {
        let token = reader.take(1)?[0];
        let tag = token & 0x0f;
        let kind = token >> 4;
        match (tag, kind) {
            (1, 1) if open.is_empty() && !output.is_empty() && reader.data.is_empty() => {
                return Some(output);
            }
            (2, 3) => {
                let name = reader.interned()?;
                let depth = open.len();
                if depth == 0 && !output.is_empty() {
                    return None;
                }
                if depth >= 256 {
                    return None;
                }
                open.push(name);
                attr_element = None;
                if metadata.collects(name, depth) {
                    if output.len() >= 65_536 {
                        return None;
                    }
                    attr_element = Some(output.len());
                    output.push(Element {
                        name,
                        depth,
                        attrs: BTreeMap::new(),
                    });
                }
            }
            (3, 3) if open.pop()? == reader.interned()? => attr_element = None,
            (4..=10, 2) => {
                reader.string()?;
                attr_element = None;
            }
            (15, _) if !open.is_empty() => {
                let name = reader.interned()?;
                let retain = attr_element.is_some() && metadata.retains(name);
                if let Some(value) = reader.value(kind, retain, value_limit)?
                    && let Some(index) = attr_element
                    && output[index].attrs.insert(name, value).is_some()
                {
                    return None;
                }
            }
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_package_reader_keeps_identity_fields_and_resolves_previously_skipped_strings() {
        let document = b"ABX\0\x10\x32\xff\xff\x00\x08packages\
            \x32\xff\xff\x00\x07package\
            \x3f\xff\xff\x00\x04path\xff\xff\x00\x0acom.member\
            \x3f\xff\xff\x00\x04name\x00\x03\
            \x6f\xff\xff\x00\x0csharedUserId\x00\x00\x27\x11\
            \x33\x00\x01\
            \x32\xff\xff\x00\x04cert\x33\x00\x06\
            \x33\x00\x00\x11";
        let tags = package_elements(document).unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[1].attrs.len(), 2);
        assert_eq!(tags[1].attrs[b"name".as_slice()], "com.member");
        assert_eq!(tags[1].attrs[b"sharedUserId".as_slice()], "10001");
        assert!(package_elements(&document[..document.len() - 1]).is_none());
    }
}
