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

use super::*;

const RULE: &str = r"||site.test/hls/$hls=||site.test\/hls\/ad.ts|";
const INPUT: &str = "#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXTINF:10,\nad.ts\n#EXTINF:10,\nmain.ts\n";

pub(super) fn fixture(req: &Request<Incoming>) -> Option<Response<Full<Bytes>>> {
    let mime = match req.uri().path().strip_prefix("/hls/")? {
        "playlist.m3u8" => "Application/Vnd.Apple.Mpegurl; charset=utf-8",
        "application.m3u8" => "application/x-mpegurl",
        "audio.m3u8" => "audio/mpegurl",
        "legacy.m3u8" => "audio/x-mpegurl",
        "plain.m3u8" => "text/plain",
        _ => return None,
    };
    Some(
        Response::builder()
            .header("etag", "\"fixture\"")
            .header("content-type", mime)
            .header("content-length", INPUT.len())
            .body(Full::new(Bytes::from_static(INPUT.as_bytes())))
            .unwrap(),
    )
}

#[tokio::test]
async fn hls_removes_segments_and_their_tags_from_decrypted_playlist_types() {
    let rig = rig(Setup {
        rules: RULE,
        ..Setup::default()
    })
    .await;
    for path in [
        "playlist.m3u8",
        "application.m3u8",
        "audio.m3u8",
        "legacy.m3u8",
    ] {
        let path = format!("/hls/{path}");
        let (headers, status, body, real) = rig
            .get_response(Trust::System, H1, &path, None)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert!(!real);
        assert_eq!(
            body,
            "#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXTINF:10,\nmain.ts\n"
        );
        assert!(!headers.contains_key("etag"));
    }
}

#[tokio::test]
async fn hls_leaves_a_matching_non_playlist_content_type_untouched() {
    let rig = rig(Setup {
        rules: RULE,
        ..Setup::default()
    })
    .await;
    let (headers, status, body, _) = rig
        .get_response(Trust::System, H1, "/hls/plain.m3u8", None)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, INPUT);
    assert_eq!(headers["etag"], "\"fixture\"");
    assert!(headers.contains_key("content-length"));
}

#[tokio::test]
async fn hls_removes_segments_before_replace_changes_the_remaining_playlist() {
    let rig = rig(Setup {
        rules: r"||site.test/hls/$replace=/ad\.ts/main.ts/
||site.test/hls/$replace=/TARGETDURATION:10/TARGETDURATION:11/
||site.test/hls/$hls=ad.ts",
        ..Setup::default()
    })
    .await;
    let (status, body, _) = rig
        .get(Trust::System, H1, "/hls/playlist.m3u8", None)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        "#EXTM3U\n#EXT-X-TARGETDURATION:11\n#EXTINF:10,\nmain.ts\n"
    );
}
