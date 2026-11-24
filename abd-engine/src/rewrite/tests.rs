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

//! Frame sequences that exercise collection, rewriting and fallback.

use std::task::Poll;

use http_body_util::Full;
use hyper::HeaderMap;
use hyper::header::HeaderValue;

use super::*;
use crate::limits::MAX_REWRITE_RESULT;
use crate::response_action::ActionSpec;

fn action(pattern: &str, text: &str, flags: bool) -> Vec<Arc<ResponseAction>> {
    vec![Arc::new(
        ActionSpec::Replace {
            pattern: pattern.to_owned(),
            replacement: text.to_owned(),
            insensitive: flags,
            single_line: flags,
        }
        .compile()
        .unwrap(),
    )]
}

fn text<B>(body: B) -> Response<B> {
    Response::builder()
        .header(CONTENT_TYPE, "Text/PLAIN; charset=utf-8")
        .body(body)
        .unwrap()
}

fn data(bytes: impl Into<Bytes>) -> Item<&'static str> {
    Ok(Frame::data(bytes.into()))
}

fn stream<I>(
    frames: I,
) -> impl Body<Data = Bytes, Error = &'static str> + Send + Sync + Unpin + 'static
where
    I: IntoIterator<Item = Item<&'static str>>,
    I::IntoIter: Send + Sync + Unpin + 'static,
{
    let mut frames = frames.into_iter();
    synthetic(move |_| Poll::Ready(frames.next()))
}

struct Synthetic<F>(F);

fn synthetic<F>(poll: F) -> Synthetic<F>
where
    F: FnMut(&mut Context<'_>) -> Poll<Option<Item<&'static str>>>,
{
    Synthetic(poll)
}

impl<F> Body for Synthetic<F>
where
    F: FnMut(&mut Context<'_>) -> Poll<Option<Item<&'static str>>> + Unpin,
{
    type Data = Bytes;
    type Error = &'static str;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Item<Self::Error>>> {
        (self.get_mut().0)(cx)
    }
}

#[tokio::test]
async fn replacement_uses_captures_both_flags_and_literal_dollars() {
    let response = wrap_with(
        text(Full::new(Bytes::from_static(b"BEFORE A\nB AFTER"))),
        action("(a).(b)", "$2$1 $0 $word", true),
        &Slots::new(1),
    );
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "BEFORE BA $0 $word AFTER"
    );
}

#[tokio::test]
async fn replacement_sees_every_frame_and_keeps_the_trailers() {
    let mut trailers = HeaderMap::new();
    trailers.insert("x-end", HeaderValue::from_static("kept"));
    let frames = (0..45).map(|_| data("x")).chain([
        data("start"),
        data(" "),
        data("end"),
        Ok(Frame::trailers(trailers.clone())),
    ]);
    let response = wrap_with(
        text(stream(frames)),
        action("start end", "changed", false),
        &Slots::new(1),
    );
    let mut body = response.into_body();
    assert_eq!(
        body.frame().await.unwrap().unwrap().into_data().unwrap(),
        format!("{}changed", "x".repeat(45))
    );
    assert_eq!(
        body.frame()
            .await
            .unwrap()
            .unwrap()
            .into_trailers()
            .unwrap(),
        trailers
    );
    assert!(body.frame().await.is_none());
}

#[tokio::test]
async fn a_body_one_byte_over_the_limit_passes_through_in_order() {
    let prefix = Bytes::from(vec![b'x'; MAX_REWRITE_BODY]);
    let frames = [data(prefix.clone()), data("y"), data("z")];
    let response = wrap_with(
        text(stream(frames)),
        action("x", "q", false),
        &Slots::new(1),
    );
    let mut body = response.into_body();
    for chunk in [prefix, Bytes::from_static(b"y"), Bytes::from_static(b"z")] {
        assert_eq!(
            body.frame().await.unwrap().unwrap().into_data().unwrap(),
            chunk
        );
    }
    assert!(body.frame().await.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_stalled_body_releases_its_original_prefix_and_streams_the_rest() {
    let mut first = Some(data("before"));
    let mut last = Some(data("after"));
    let end = tokio::time::Instant::now() + REWRITE_WAIT + std::time::Duration::from_secs(1);
    let mut wait = Box::pin(tokio::time::sleep_until(end));
    let inner = synthetic(move |cx| {
        if first.is_some() {
            return Poll::Ready(first.take());
        }
        wait.as_mut().poll(cx).map(|_| last.take())
    });
    let response = wrap_with(
        text(inner),
        action("before", "changed", false),
        &Slots::new(1),
    );
    let mut body = response.into_body();
    assert_eq!(
        body.frame().await.unwrap().unwrap().into_data().unwrap(),
        "before"
    );
    assert!(tokio::time::Instant::now() < end);
    assert_eq!(
        body.frame().await.unwrap().unwrap().into_data().unwrap(),
        "after"
    );
}

#[tokio::test]
async fn an_error_follows_the_original_data() {
    let frames = [data("be"), data("fore"), Err("upstream error")];
    let response = wrap_with(
        text(stream(frames)),
        action("before", "changed", false),
        &Slots::new(1),
    );
    let mut body = response.into_body();
    assert_eq!(
        body.frame().await.unwrap().unwrap().into_data().unwrap(),
        "before"
    );
    assert_eq!(body.frame().await.unwrap().unwrap_err(), "upstream error");
}

#[tokio::test]
async fn a_result_over_the_limit_discards_even_preceding_changes() {
    let mut bytes = vec![b'x'; MAX_REWRITE_RESULT / 3 + 1];
    bytes[0] = b'z';
    let original = Bytes::from(bytes);
    let mut actions = action("z", "x", false);
    actions.extend(action("(x+)", "$1$1$1", false));
    let response = wrap_with(text(Full::new(original.clone())), actions, &Slots::new(1));
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        original
    );
}

#[tokio::test]
async fn the_fifth_response_is_passed_through_without_waiting() {
    let slots = Slots::new(MAX_REWRITES);
    let response = || {
        let body = Full::new(Bytes::from_static(b"X"));
        let mut response = text(body);
        for (name, value) in [
            ("content-length", "1"),
            ("accept-ranges", "bytes"),
            ("etag", "\"original\""),
            ("content-md5", "abc"),
            ("digest", "sha-256=abc"),
        ] {
            response
                .headers_mut()
                .insert(name, HeaderValue::from_static(value));
        }
        wrap_with(response, action("X", "changed", false), &slots)
    };
    let held: Vec<_> = (0..MAX_REWRITES).map(|_| response()).collect();
    let passed = response();
    assert_eq!(passed.headers().len(), 6);
    assert_eq!(passed.into_body().collect().await.unwrap().to_bytes(), "X");
    for response in held {
        assert_eq!(response.headers().len(), 1);
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "changed"
        );
    }
    assert_eq!(response().headers().len(), 1);
}

#[test]
fn only_eligible_text_responses_are_buffered() {
    for mime in [
        "text/html",
        "application/json",
        "application/javascript",
        "application/x-javascript",
        "application/xml",
        "application/xhtml+xml",
        "application/x-www-form-urlencoded",
        "application/vnd.apple.mpegurl",
        "application/x-mpegurl",
        "application/ld+json",
        "application/rss+xml",
    ] {
        assert!(
            takes(
                &Response::builder()
                    .header(CONTENT_TYPE, mime)
                    .body(())
                    .unwrap()
            ),
            "{mime}"
        );
    }
    for (status, mime, coding, size) in [
        (204, "text/plain", "identity", "0"),
        (206, "text/plain", "identity", "10"),
        (302, "text/plain", "identity", "10"),
        (200, "image/png", "identity", "10"),
        (200, "text/plain", "identity, gzip", "10"),
        (200, "text/plain", "identity", "2097153"),
    ] {
        let response = Response::builder()
            .status(status)
            .header(CONTENT_TYPE, mime)
            .header(CONTENT_ENCODING, coding)
            .header(CONTENT_LENGTH, size)
            .body(())
            .unwrap();
        assert!(!takes(&response), "{response:?}");
    }
}

/// Let a blocking job start and hold it until the test releases it.
async fn held_work(
    slots: &Slots,
) -> (
    Rewrite<Full<Bytes>>,
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<bool>,
) {
    use tokio::sync::oneshot::channel;

    let mut body = Rewrite::new(
        Full::new(Bytes::new()),
        action("original", "changed", false),
        slots.responses.clone().try_acquire_owned().unwrap(),
        slots.workers.clone(),
    );
    let State::Collecting(mut collected) = std::mem::replace(&mut body.state, State::Ended) else {
        unreachable!()
    };
    collected.bytes = b"original".to_vec();
    let worker = slots.workers.clone().try_acquire_owned().unwrap();
    let (began, started) = channel();
    let (release, wait) = channel();
    let (report, reported) = channel();
    body.state = State::Rewriting(Work::start(
        collected,
        worker,
        move |input, actions, check| {
            began.send(()).ok()?;
            wait.blocking_recv().ok()?;
            report.send(check()).ok()?;
            apply_actions(input, actions, check)
        },
    ));
    tokio::time::timeout(REWRITE_WORK, started)
        .await
        .unwrap()
        .unwrap();
    (body, release, reported)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polling_the_body_yields_until_the_blocking_job_hands_back_its_result() {
    let slots = Slots::new(1);
    let (mut body, release, result) = held_work(&slots).await;
    let mut cx = Context::from_waker(std::task::Waker::noop());
    assert!(Pin::new(&mut body).poll_frame(&mut cx).is_pending());
    assert_eq!(slots.responses.available_permits(), 0);
    release.send(()).unwrap();
    assert!(result.await.unwrap());
    assert_eq!(
        body.frame().await.unwrap().unwrap().into_data().unwrap(),
        "changed"
    );
    assert_eq!(slots.responses.available_permits(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_body_releases_its_slot_and_cancels_further_searches() {
    let slots = Slots::new(1);
    let (body, release, result) = held_work(&slots).await;
    drop(body);
    assert_eq!(slots.responses.available_permits(), 1);
    assert_eq!(slots.workers.available_permits(), 0);
    release.send(()).unwrap();
    assert!(!result.await.unwrap());
    let worker = tokio::time::timeout(REWRITE_WORK, slots.workers.acquire())
        .await
        .unwrap();
    assert!(worker.is_ok());
}

#[tokio::test(start_paused = true)]
async fn a_timed_out_rewrite_returns_the_original_before_the_worker_exits() {
    let slots = Slots::new(1);
    let (mut body, release, result) = held_work(&slots).await;
    tokio::time::advance(REWRITE_WORK + std::time::Duration::from_millis(1)).await;
    assert_eq!(
        body.frame().await.unwrap().unwrap().into_data().unwrap(),
        "original"
    );
    assert_eq!(slots.responses.available_permits(), 1);
    assert_eq!(slots.workers.available_permits(), 0);
    release.send(()).unwrap();
    assert!(!result.await.unwrap());
}

#[cfg(panic = "unwind")]
#[tokio::test]
async fn a_worker_that_panics_returns_the_original() {
    let slots = Slots::new(1);
    let mut body = Rewrite::new(
        Full::new(Bytes::new()),
        action("original", "changed", false),
        slots.responses.clone().try_acquire_owned().unwrap(),
        slots.workers.clone(),
    );
    let State::Collecting(mut collected) = std::mem::replace(&mut body.state, State::Ended) else {
        unreachable!()
    };
    collected.bytes = b"original".to_vec();
    let worker = slots.workers.clone().try_acquire_owned().unwrap();
    body.state = State::Rewriting(Work::start(collected, worker, |_, _, _| {
        panic!("failed search")
    }));
    assert_eq!(
        body.frame().await.unwrap().unwrap().into_data().unwrap(),
        "original"
    );
    assert_eq!(slots.responses.available_permits(), 1);
}

#[tokio::test]
async fn a_collected_json_body_is_pruned_before_replacement() {
    let prune = ActionSpec::Prune("$.ad".to_owned()).compile().unwrap();
    let mut actions = action("ad|original", "changed", false);
    actions.insert(0, Arc::new(prune));
    let body = stream([data(r#"{"ad":tr"#), data(r#"ue,"keep":"original"}"#)]);
    let response = Response::builder()
        .header(CONTENT_TYPE, "application/json")
        .body(body)
        .unwrap();
    let response = wrap_with(response, actions, &Slots::new(1));
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        r#"{"keep":"changed"}"#
    );
}
