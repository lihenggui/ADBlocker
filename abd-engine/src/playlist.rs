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

//! Segment edits on UTF-8 playlists. The only rewritten line is a kept
//! byte-range tag whose implicit offset depended on a removed predecessor
//! (RFC 8216 §4.3.2.2). Other surviving lines are copied verbatim.

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

struct ByteRange<'a> {
    line: usize,
    value: &'a str,
}

impl ByteRange<'_> {
    fn bounds(&self, preceding_end: Option<u64>) -> Option<(u64, u64)> {
        let decimal = |text: &str| {
            text.bytes()
                .all(|byte| byte.is_ascii_digit())
                .then(|| text.parse::<u64>().ok())
                .flatten()
        };
        let (length, offset) = match self.value.split_once('@') {
            Some((length, offset)) => (decimal(length)?, decimal(offset)?),
            None => (decimal(self.value)?, preceding_end?),
        };
        Some((offset, offset.checked_add(length)?))
    }
}

struct PreviousSegment<'a> {
    uri: &'a str,
    end: Option<u64>,
    removed: bool,
}

#[derive(Clone, Copy)]
enum LineEdit {
    Keep,
    Remove,
    AddOffset(u64),
}

impl LineEdit {
    fn removal(remove: bool) -> Self {
        if remove { Self::Remove } else { Self::Keep }
    }
}

#[derive(Default)]
struct SegmentTags<'a> {
    next: Vec<usize>,
    next_matches: bool,
    byte_range: Option<ByteRange<'a>>,
    ambiguous_range: bool,
    ranges: [Option<RangingTag>; Range::UplynkKey as usize + 1],
}

impl<'a> SegmentTags<'a> {
    fn add(
        &mut self,
        index: usize,
        text: &'a str,
        matches: &impl Fn(&str) -> Option<bool>,
    ) -> Option<()> {
        let name = if text.starts_with("#EXTINF,") {
            // The reference's worked playlist uses a comma after EXTINF.
            "#EXTINF"
        } else {
            text.split_once(':').map_or(text, |(name, _)| name)
        };
        match tag(name) {
            Some(kind @ (Tag::Next | Tag::ByteRange)) => {
                self.next.push(index);
                self.next_matches = self.next_matches || matches(text)?;
                if matches!(kind, Tag::ByteRange) {
                    let value = text.split_once(':').map_or("", |(_, value)| value);
                    self.ambiguous_range |= self
                        .byte_range
                        .replace(ByteRange { line: index, value })
                        .is_some();
                }
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
        edits: &mut [LineEdit],
    ) -> Option<bool> {
        let byte_range = self.byte_range.is_some();
        let applies = |kind| kind != Range::Bitrate as usize || !byte_range;
        let remove =
            self.next_matches
                || self.ranges.iter().enumerate().any(|(kind, tag)| {
                    applies(kind) && tag.as_ref().is_some_and(|tag| tag.matches)
                })
                || url_matches(line)?;
        edits[index] = LineEdit::removal(remove);
        for index in self.next.drain(..) {
            edits[index] = LineEdit::removal(remove);
        }
        for (kind, tag) in self.ranges.iter_mut().enumerate() {
            if applies(kind)
                && let Some(tag) = tag
            {
                tag.used_by_survivor |= !remove;
                edits[tag.line] = LineEdit::removal(!tag.used_by_survivor);
            }
        }
        self.next_matches = false;
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
    let mut edits = vec![LineEdit::Keep; count];
    let mut tags = SegmentTags::default();
    let mut previous: Option<PreviousSegment<'_>> = None;
    let mut changed = false;
    for (index, line) in lines.clone().enumerate().skip(1) {
        if !within_budget() {
            return None;
        }
        let line = text(line);
        if line.starts_with('#') {
            tags.add(index, line, &tag_matches)?;
        } else if !line.is_empty() {
            let removed = tags.segment(index, line, &url_matches, &mut edits)?;
            let byte_range = tags.byte_range.take();
            // A segment can only have one EXT-X-BYTERANGE.
            if std::mem::take(&mut tags.ambiguous_range) {
                return Some(Cow::Borrowed(body));
            }
            let preceding_end = previous
                .as_ref()
                .filter(|segment| segment.uri == line)
                .and_then(|segment| segment.end);
            let bounds = byte_range
                .as_ref()
                .and_then(|range| range.bounds(preceding_end));
            if let Some(range) = byte_range
                && !range.value.contains('@')
                && !removed
                && previous.as_ref().is_some_and(|segment| segment.removed)
            {
                let Some((offset, _)) = bounds else {
                    return Some(Cow::Borrowed(body));
                };
                edits[range.line] = LineEdit::AddOffset(offset);
            }
            previous = Some(PreviousSegment {
                uri: line,
                end: bounds.map(|(_, end)| end),
                removed,
            });
            changed |= removed;
        }
    }
    if !changed {
        return Some(Cow::Borrowed(body));
    }
    build_output(body, lines, edits, within_budget).map(Cow::Owned)
}

fn build_output<'a>(
    body: &[u8],
    lines: impl Iterator<Item = &'a str>,
    edits: Vec<LineEdit>,
    within_budget: &dyn Fn() -> bool,
) -> Option<Vec<u8>> {
    let mut edited = Vec::with_capacity(body.len());
    for (line, edit) in lines.zip(edits) {
        if !within_budget() {
            return None;
        }
        match edit {
            LineEdit::Keep => edited.extend_from_slice(line.as_bytes()),
            LineEdit::Remove => (),
            LineEdit::AddOffset(offset) => {
                let (tag, ending) = line.split_at(text(line).len());
                edited.extend_from_slice(tag.as_bytes());
                edited.push(b'@');
                edited.extend_from_slice(offset.to_string().as_bytes());
                edited.extend_from_slice(ending.as_bytes());
            }
        }
    }
    Some(edited)
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
    fn a_kept_range_preserves_its_offset_after_the_first_segment_is_removed() {
        let input = "#EXTM3U\n#EXT-X-VERSION:4\n#EXT-X-TARGETDURATION:10\n\
            #EXTINF:10,promotion\n#EXT-X-BYTERANGE:100@0\nclip.ts\n\
            #EXTINF:10,article\n#EXT-X-BYTERANGE:100\nclip.ts\n#EXT-X-ENDLIST\n";
        let result = edit(
            input.as_bytes(),
            |tag| Some(tag.ends_with(",promotion")),
            |_| Some(false),
            &|| true,
        );
        assert_eq!(
            result.as_deref(),
            Some(
                &b"#EXTM3U\n#EXT-X-VERSION:4\n#EXT-X-TARGETDURATION:10\n\
                #EXTINF:10,article\n#EXT-X-BYTERANGE:100@100\nclip.ts\n#EXT-X-ENDLIST\n"[..]
            )
        );
    }

    #[test]
    fn a_kept_range_accounts_for_a_removed_implicit_range() {
        let input = "#EXTM3U\n#EXT-X-BYTERANGE:12@100\nclip.ts\n\
            #EXTINF:10,ad\n#EXT-X-BYTERANGE:13\nclip.ts\n\
            #EXT-X-BYTERANGE:14\r\nclip.ts\n";
        assert_eq!(
            remove_ads(input).as_ref(),
            b"#EXTM3U\n#EXT-X-BYTERANGE:12@100\nclip.ts\n#EXT-X-BYTERANGE:14@125\r\nclip.ts\n"
        );
    }

    #[test]
    fn a_kept_range_with_no_known_offset_leaves_the_playlist_intact() {
        for input in [
            "#EXTM3U\n#EXTINF:10,ad\nclip.ts\n#EXT-X-BYTERANGE:100\nclip.ts\n",
            "#EXTM3U\n#EXT-X-BYTERANGE:12@0\nother.ts\n\
             #EXTINF:10,ad\n#EXT-X-BYTERANGE:13\nclip.ts\n\
             #EXT-X-BYTERANGE:14\nclip.ts\n",
        ] {
            assert_eq!(remove_ads(input).as_ref(), input.as_bytes());
        }
    }

    #[test]
    fn an_implicit_range_after_a_kept_segment_keeps_its_original_tag() {
        let input = "#EXTM3U\n#EXT-X-BYTERANGE:12@0\nclip.ts\n\
            #EXT-X-BYTERANGE:13\nclip.ts\nad.ts\n";
        assert_eq!(
            remove_ads(input).as_ref(),
            b"#EXTM3U\n#EXT-X-BYTERANGE:12@0\nclip.ts\n#EXT-X-BYTERANGE:13\nclip.ts\n"
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

    mod properties {
        use proptest::prelude::*;

        use super::*;
        use crate::test_support::{arbitrary_bytes, assert_removal, mutate};

        fn playlist() -> impl Strategy<Value = Vec<u8>> {
            let segment = (
                prop::sample::select(vec![
                    "#EXTINF:4,",
                    "#EXT-X-KEY:METHOD=AES-128,URI=\"key\"",
                    "#EXT-X-MAP:URI=\"init.mp4\"",
                    "#EXT-X-BYTERANGE:12@0",
                    "#EXT-X-PART:DURATION=0.2,URI=\"ad.ts\"",
                    "#comment",
                ]),
                "[a-z]{1,10}",
                any::<bool>(),
            );
            let structured = (
                prop::collection::vec(segment, 0..18),
                any::<bool>(),
                any::<bool>(),
                prop::option::of((any::<prop::sample::Index>(), prop::option::of(any::<u8>()))),
            )
                .prop_map(|(segments, bom, final_newline, mutation)| {
                    let mut text = if bom {
                        String::from("\u{feff}")
                    } else {
                        String::new()
                    };
                    text.push_str("#EXTM3U");
                    for (tag, name, crlf) in segments {
                        let newline = if crlf { "\r\n" } else { "\n" };
                        text.push_str(&format!("{newline}{tag}{newline}{name}.ts"));
                    }
                    if final_newline {
                        text.push('\n');
                    }
                    mutate(text.into_bytes(), mutation)
                });
            prop_oneof![arbitrary_bytes(), structured]
        }

        proptest! {
            #[test]
            fn editing_returns_a_subsequence_of_the_original_bytes(
                input in playlist(), needle in prop::sample::select(vec!["ad", ".ts", "KEY", "PART", "MAP"]),
            ) {
                let matches = |text: &str| Some(text.contains(needle));
                if let Some(output) = edit(&input, matches, matches, &|| true) {
                    assert_removal(&input, &output)?;
                }
            }

            #[test]
            fn a_playlist_without_any_matches_is_preserved_byte_for_byte(input in playlist()) {
                let output = edit(&input, |_| Some(false), |_| Some(false), &|| true);
                prop_assert_eq!(output.as_deref(), Some(input.as_slice()));
            }
        }
    }
}
