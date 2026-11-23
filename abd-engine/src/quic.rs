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

//! Recognize client QUIC Initials before refusing them in the VPN.

const MAX_CID: usize = 20;

fn take<'a>(bytes: &mut &'a [u8], count: usize) -> Option<&'a [u8]> {
    let (prefix, rest) = bytes.split_at_checked(count)?;
    *bytes = rest;
    Some(prefix)
}

fn varint(bytes: &mut &[u8]) -> Option<usize> {
    let first = *bytes.first()?;
    let field = take(bytes, 1 << (first >> 6))?;
    field[1..].iter().try_fold(usize::from(first & 63), |v, b| {
        v.checked_mul(256)?.checked_add(usize::from(*b))
    })
}

fn id<'a>(bytes: &mut &'a [u8]) -> Option<&'a [u8]> {
    let len = usize::from(*take(bytes, 1)?.first()?);
    (len <= MAX_CID).then_some(())?;
    take(bytes, len)
}

// The supported versions have different long-header Initial packet types.
const V1_INITIAL: (u32, u8) = (1, 0xc);
const V2_INITIAL: (u32, u8) = (0x6b33_43cf, 0xd);

fn parse_initial(datagram: &[u8]) -> Option<()> {
    if datagram.len() < 1200 {
        return None;
    }
    let wire = u32::from_be_bytes(datagram[1..5].try_into().ok()?);
    match (wire, datagram[0] >> 4) {
        V1_INITIAL | V2_INITIAL | (0xff00_001d..=0xff00_0022, 0xc) => {}
        _ => return None,
    }
    let mut bytes = &datagram[5..];
    if id(&mut bytes)?.len() < 8 {
        return None;
    }
    id(&mut bytes)?;
    let token_len = varint(&mut bytes)?;
    take(&mut bytes, token_len)?;
    let packet_len = varint(&mut bytes)?;
    let protected = take(&mut bytes, packet_len)?;
    // Header protection samples 16 bytes starting four bytes after the
    // packet number offset.
    (protected.len() >= 20).then_some(())
}

/// A recognized client Initial, including its complete length and CID fields.
pub fn is_initial(datagram: &[u8]) -> bool {
    parse_initial(datagram).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_complete_supported_client_initials_are_recognized() {
        let mut packet = vec![0; 1200];
        packet[5] = 8;
        packet[6..14].copy_from_slice(b"original");
        // Empty source CID and token, then the length of the protected packet.
        packet[16..18].copy_from_slice(&(0x4000u16 | 1182).to_be_bytes());
        for (version, kind) in [V1_INITIAL, V2_INITIAL, (0xff00_001d, 0xc)] {
            packet[0] = kind << 4;
            packet[1..5].copy_from_slice(&version.to_be_bytes());
            assert!(is_initial(&packet));
            assert!(!is_initial(&packet[..1199]));
            packet[0] &= !0x40;
            assert!(!is_initial(&packet));
        }
        packet[0] = 0xc0;
        packet[1..5].copy_from_slice(&0xdead_beefu32.to_be_bytes());
        assert!(!is_initial(&packet));
        packet[0] = 0x40;
        assert!(!is_initial(&packet));
        assert!(!is_initial(&[0xaa; 1200]));
    }
}
