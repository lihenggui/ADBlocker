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

//! Segment edits on UTF-8 playlists, preserving every surviving line verbatim.

use std::borrow::Cow;

use crate::limits::MAX_PLAYLIST_LINES;

pub(crate) mod uri;

#[derive(Clone, Copy)]
enum Range {
    Key,
    Map,
    Bitrate,
    UplynkSegment,
    UplynkKey,
}

enum Tag {
    Next,
    ByteRange,
    Range(Range),
    Playlist,
}

fn tag(name: &str) -> Option<Tag> {
    use Range::{Bitrate, Key, Map, UplynkKey, UplynkSegment};
    Some(match name {
        // RFC 8216 §§4.3.2.1–3, 4.3.2.6, 4.3.4.2.
        "#EXTINF" | "#EXT-X-DISCONTINUITY" | "#EXT-X-PROGRAM-DATE-TIME" | "#EXT-X-STREAM-INF" => {
            Tag::Next
        }
        "#EXT-X-BYTERANGE" => Tag::ByteRange,
        // 8216bis-22 §§4.4.4.7, 4.4.4.9 and §3.2 (parts of the next parent).
        "#EXT-X-GAP" | "#EXT-X-PART" => Tag::Next,
        // RFC 8216 §§4.3.2.4–5. A later instance of the same name ends the range.
        "#EXT-X-KEY" => Tag::Range(Key),
        "#EXT-X-MAP" => Tag::Range(Map),
        // 8216bis-22 §4.4.4.8 excludes segments with EXT-X-BYTERANGE.
        "#EXT-X-BITRATE" => Tag::Range(Bitrate),
        // AdGuard does not publish its non-standard tag list. Use the two
        // tags whose scope is shown in the filter reference's example.
        "#UPLYNK-SEGMENT" => Tag::Range(UplynkSegment),
        "#UPLYNK-KEY" => Tag::Range(UplynkKey),
        // RFC 8216 §§4.3.1, 4.3.3, 4.3.4 (except STREAM-INF), 4.3.5.
        "#EXTM3U"
        | "#EXT-X-VERSION"
        | "#EXT-X-TARGETDURATION"
        | "#EXT-X-MEDIA-SEQUENCE"
        | "#EXT-X-DISCONTINUITY-SEQUENCE"
        | "#EXT-X-ENDLIST"
        | "#EXT-X-PLAYLIST-TYPE"
        | "#EXT-X-I-FRAMES-ONLY"
        | "#EXT-X-MEDIA"
        | "#EXT-X-I-FRAME-STREAM-INF"
        | "#EXT-X-SESSION-DATA"
        | "#EXT-X-SESSION-KEY"
        | "#EXT-X-INDEPENDENT-SEGMENTS"
        | "#EXT-X-START"
        // 8216bis-22 §§4.4.2.3, 4.4.3.7–8, 4.4.6.6.
        | "#EXT-X-DEFINE"
        | "#EXT-X-PART-INF"
        | "#EXT-X-SERVER-CONTROL"
        | "#EXT-X-CONTENT-STEERING"
        // 8216bis-22 §4.4.5. These describe time ranges, omitted or hinted
        // resources, or other renditions, outside the URL-line tag scopes.
        | "#EXT-X-DATERANGE"
        | "#EXT-X-SKIP"
        | "#EXT-X-PRELOAD-HINT"
        | "#EXT-X-RENDITION-REPORT" => Tag::Playlist,
        _ => return None,
    })
}

struct RangingTag {
    line: usize,
    matches: bool,
    used_by_survivor: bool,
}

#[derive(Default)]
struct SegmentTags {
    next: Vec<usize>,
    next_matches: bool,
    byte_range: bool,
    ranges: [Option<RangingTag>; Range::UplynkKey as usize + 1],
}

impl SegmentTags {
    fn add(
        &mut self,
        index: usize,
        text: &str,
        matches: &impl Fn(&str) -> Option<bool>,
    ) -> Option<()> {
        let name = if text.starts_with("#EXTINF,") {
            // The reference's worked playlist uses a comma after EXTINF.
            "#EXTINF"
        } else {
            text.split_once(':').map_or(text, |(name, _)| name)
        };
        match tag(name) {
            Some(Tag::Next | Tag::ByteRange) => {
                self.next.push(index);
                self.next_matches = self.next_matches || matches(text)?;
                self.byte_range |= name == "#EXT-X-BYTERANGE";
            }
            Some(Tag::Range(range)) => {
                self.ranges[range as usize] = Some(RangingTag {
                    line: index,
                    matches: matches(text)?,
                    used_by_survivor: false,
                });
            }
            Some(Tag::Playlist) | None => (),
        }
        Some(())
    }

    fn segment(
        &mut self,
        index: usize,
        line: &str,
        url_matches: &impl Fn(&str) -> Option<bool>,
        discard: &mut [bool],
    ) -> Option<bool> {
        let byte_range = self.byte_range;
        let applies = |kind| kind != Range::Bitrate as usize || !byte_range;
        let remove =
            self.next_matches
                || self.ranges.iter().enumerate().any(|(kind, tag)| {
                    applies(kind) && tag.as_ref().is_some_and(|tag| tag.matches)
                })
                || url_matches(line)?;
        discard[index] = remove;
        for index in self.next.drain(..) {
            discard[index] = remove;
        }
        for (kind, tag) in self.ranges.iter_mut().enumerate() {
            if applies(kind)
                && let Some(tag) = tag
            {
                tag.used_by_survivor |= !remove;
                discard[tag.line] = !tag.used_by_survivor;
            }
        }
        self.next_matches = false;
        self.byte_range = false;
        Some(remove)
    }
}

fn text(line: &str) -> &str {
    line.strip_suffix('\n')
        .map_or(line, |line| line.strip_suffix('\r').unwrap_or(line))
}

/// A cancelled edit or a playlist over the line limit returns None so the
/// response can pass through. An unchanged playlist borrows the input.
pub(crate) fn edit<'a>(
    body: &'a [u8],
    tag_matches: impl Fn(&str) -> Option<bool>,
    url_matches: impl Fn(&str) -> Option<bool>,
    within_budget: &dyn Fn() -> bool,
) -> Option<Cow<'a, [u8]>> {
    let Ok(playlist) = std::str::from_utf8(body) else {
        return Some(Cow::Borrowed(body));
    };
    let lines = playlist.split_inclusive('\n');
    let first = text(lines.clone().next().unwrap_or(""));
    if first.strip_prefix('\u{feff}').unwrap_or(first) != "#EXTM3U" {
        return Some(Cow::Borrowed(body));
    }
    let count = lines.clone().take(MAX_PLAYLIST_LINES + 1).count();
    if count > MAX_PLAYLIST_LINES {
        return None;
    }
    let mut discard = vec![false; count];
    let mut tags = SegmentTags::default();
    let mut changed = false;
    for (index, line) in lines.clone().enumerate().skip(1) {
        if !within_budget() {
            return None;
        }
        let line = text(line);
        if line.starts_with('#') {
            tags.add(index, line, &tag_matches)?;
        } else if !line.is_empty() {
            changed |= tags.segment(index, line, &url_matches, &mut discard)?;
        }
    }
    if !changed {
        return Some(Cow::Borrowed(body));
    }
    let mut edited = Vec::with_capacity(body.len());
    for (line, discard) in lines.zip(discard) {
        if !within_budget() {
            return None;
        }
        if !discard {
            edited.extend_from_slice(line.as_bytes());
        }
    }
    Some(Cow::Owned(edited))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remove_ads(input: &str) -> Cow<'_, [u8]> {
        edit(
            input.as_bytes(),
            |line| Some(line.contains("ad")),
            |line| Some(line.starts_with("ad")),
            &|| true,
        )
        .unwrap()
    }

    #[test]
    fn one_segment_tags_are_removed_but_comments_and_playlist_tags_survive() {
        let input = "#EXTM3U\r\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:10\r\n\r\n\
            #EXT-X-PROGRAM-DATE-TIME:2025-01-01T00:00:00Z\n#EXT-X-DISCONTINUITY\n\
            #EXTINF:1,ad\n#EXT-X-UNRECOGNIZED:ad\r\n# ad comment\r\n#EXT-X-GAP\n\
            #EXT-X-BYTERANGE:100@0\n#EXT-X-PART:URI=\"ad-part.ts\"\nfirst.ts\n\
            #EXTINF:10,\r\nkeep.ts\n#EXT-X-ENDLIST";
        assert_eq!(
            remove_ads(input).as_ref(),
            b"#EXTM3U\r\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:10\r\n\r\n\
              #EXT-X-UNRECOGNIZED:ad\r\n# ad comment\r\n#EXTINF:10,\r\nkeep.ts\n#EXT-X-ENDLIST"
        );
    }

    #[test]
    fn a_ranging_tag_survives_if_any_segment_in_its_range_survives() {
        let input = "#EXTM3U\n#EXT-X-KEY:ad\n#EXT-X-MAP:ad\nfirst.ts\nsecond.ts\n\
            #EXT-X-KEY:main\n#EXT-X-MAP:main\nad.ts\nkeep.ts\n#EXT-X-KEY:next\n";
        assert_eq!(
            remove_ads(input).as_ref(),
            b"#EXTM3U\n#EXT-X-KEY:main\n#EXT-X-MAP:main\nkeep.ts\n#EXT-X-KEY:next\n"
        );
    }

    #[test]
    fn bitrate_ranges_exclude_segments_with_a_byte_range() {
        let input = "#EXTM3U\n#EXT-X-BITRATE:ad\nfirst.ts\n#EXT-X-BYTERANGE:10@0\nkeep.ts\n\
            second.ts\n#EXT-X-BITRATE:main\nad.ts\nend.ts\n";
        assert_eq!(
            remove_ads(input).as_ref(),
            b"#EXTM3U\n#EXT-X-BYTERANGE:10@0\nkeep.ts\n#EXT-X-BITRATE:main\nend.ts\n"
        );
    }

    #[test]
    fn a_master_playlist_keeps_renditions_and_iframe_variants_in_tags() {
        let input = "\u{feff}#EXTM3U\n#EXT-X-MEDIA:URI=\"ad.m3u8\"\n\
            #EXT-X-I-FRAME-STREAM-INF:URI=\"ad.m3u8\"\n\
            #EXT-X-STREAM-INF:BANDWIDTH=100\nad.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=200\nkeep.m3u8\n";
        assert_eq!(
            remove_ads(input).as_ref(),
            "\u{feff}#EXTM3U\n#EXT-X-MEDIA:URI=\"ad.m3u8\"\n\
              #EXT-X-I-FRAME-STREAM-INF:URI=\"ad.m3u8\"\n\
              #EXT-X-STREAM-INF:BANDWIDTH=200\nkeep.m3u8\n"
                .as_bytes()
        );
    }

    #[test]
    fn skipped_or_unmatched_playlists_return_the_same_bytes() {
        for (input, remove) in [
            (&b"#EXTM3U\nad.ts\n\xff"[..], true),
            (&b"\n#EXTM3U\nad.ts\n"[..], true),
            (&b"#EXTM3U:invalid\nad.ts\n"[..], true),
            (&b"\xef\xbb\xbf#EXTM3U\r\n#EXT-X-KEY:ad\nkeep.ts"[..], false),
        ] {
            let result = edit(input, |_| Some(remove), |_| Some(remove), &|| true).unwrap();
            assert!(matches!(result, Cow::Borrowed(bytes) if std::ptr::eq(bytes, input)));
        }
    }

    #[test]
    fn the_line_bound_is_inclusive_and_larger_playlists_are_skipped() {
        let mut input = "#EXTM3U\nad.ts\n".to_owned();
        input.push_str(&"\n".repeat(MAX_PLAYLIST_LINES - 2));
        assert!(matches!(remove_ads(&input), Cow::Owned(_)));
        input.push('\n');
        assert!(edit(input.as_bytes(), |_| Some(true), |_| Some(true), &|| true).is_none());
    }

    #[test]
    fn exhaustion_partway_through_an_edit_abandons_all_removals() {
        let remaining = std::cell::Cell::new(1u8);
        assert!(
            edit(
                b"#EXTM3U\nad.ts\nkeep.ts",
                |_| Some(true),
                |_| Some(true),
                &|| {
                    let left = remaining.get();
                    remaining.set(left.saturating_sub(1));
                    left != 0
                }
            )
            .is_none()
        );
    }
}
