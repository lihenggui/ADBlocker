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

//! Collect eligible text bodies before applying response rules. A body
//! that is too large, stalls, or cannot be rewritten is sent as it came.
//! No part of a body is changed before the whole result is ready.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::task::{Context, Poll, ready};
use std::time::Instant;

use bytes::Bytes;
use http_body_util::{BodyExt, combinators::BoxBody};
use hyper::body::{Body, Frame};
use hyper::header::{ACCEPT_RANGES, CONTENT_ENCODING, CONTENT_LENGTH, ETAG};
use hyper::{Response, StatusCode};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;
use tokio::time::{Sleep, sleep};

use crate::limits::{MAX_REWRITE_BODY, MAX_REWRITES, REWRITE_WAIT, REWRITE_WORK};
use crate::rule::action::ResponseAction;

static SLOTS: std::sync::LazyLock<Slots> = std::sync::LazyLock::new(|| Slots::new(MAX_REWRITES));

/// A response can give up its place while a search still holds a worker.
/// The worker's place is returned only when the task exits.
struct Slots {
    responses: Arc<Semaphore>,
    workers: Arc<Semaphore>,
}

impl Slots {
    fn new(count: usize) -> Self {
        Self {
            responses: Arc::new(Semaphore::new(count)),
            workers: Arc::new(Semaphore::new(count)),
        }
    }
}

/// Types whose bodies a response rule can treat as text.
fn text_type(value: &str) -> bool {
    let mime = value.to_ascii_lowercase();
    let Some((group, subtype)) = mime.split_once('/') else {
        return false;
    };
    group == "text"
        || subtype.ends_with("+json")
        || subtype.ends_with("+xml")
        || group == "application"
            && matches!(
                subtype,
                "json"
                    | "javascript"
                    | "x-javascript"
                    | "xml"
                    | "xhtml+xml"
                    | "x-www-form-urlencoded"
                    | "vnd.apple.mpegurl"
                    | "x-mpegurl"
            )
}

/// Require a whole, unencoded response whose declared size can fit.
fn takes<B>(response: &Response<B>) -> bool {
    let status = response.status();
    let headers = response.headers();
    status.is_success()
        && !matches!(status, StatusCode::NO_CONTENT | StatusCode::PARTIAL_CONTENT)
        && headers.get_all(CONTENT_ENCODING).iter().all(|value| {
            value.to_str().is_ok_and(|value| {
                value
                    .split(',')
                    .all(|coding| coding.trim().eq_ignore_ascii_case("identity"))
            })
        })
        && headers.get_all(CONTENT_LENGTH).iter().all(|value| {
            value
                .to_str()
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .is_some_and(|size| size <= MAX_REWRITE_BODY)
        })
        && crate::media_type::read(headers).is_some_and(text_type)
}

/// Leave the response intact if it cannot be collected right away.
pub(crate) fn wrap<B>(
    response: Response<B>,
    actions: Vec<Arc<ResponseAction>>,
) -> Response<BoxBody<Bytes, B::Error>>
where
    B: Body<Data = Bytes> + Send + Sync + Unpin + 'static,
    B::Error: Send + Sync + Unpin + 'static,
{
    wrap_with(response, actions, &SLOTS)
}

fn wrap_with<B>(
    response: Response<B>,
    actions: Vec<Arc<ResponseAction>>,
    slots: &Slots,
) -> Response<BoxBody<Bytes, B::Error>>
where
    B: Body<Data = Bytes> + Send + Sync + Unpin + 'static,
    B::Error: Send + Sync + Unpin + 'static,
{
    if actions.is_empty() || !takes(&response) {
        return response.map(BodyExt::boxed);
    }
    let Ok(slot) = slots.responses.clone().try_acquire_owned() else {
        return response.map(BodyExt::boxed);
    };
    let (mut parts, body) = response.into_parts();
    for name in [CONTENT_LENGTH, ACCEPT_RANGES, ETAG] {
        parts.headers.remove(name);
    }
    parts.headers.remove("content-md5");
    parts.headers.remove("digest");
    Response::from_parts(
        parts,
        Rewrite::new(body, actions, slot, slots.workers.clone()).boxed(),
    )
}

/// The original bytes and any trailer, held until the upstream body ends.
struct Collection {
    bytes: Vec<u8>,
    trailers: Option<Frame<Bytes>>,
    timer: Pin<Box<Sleep>>,
    actions: Vec<Arc<ResponseAction>>,
    slot: OwnedSemaphorePermit,
}

/// An ended body whose result is still being searched. The task and the
/// fallback share the original allocation.
struct Work {
    original: Bytes,
    trailers: Option<Frame<Bytes>>,
    task: JoinHandle<Option<Vec<u8>>>,
    timer: Pin<Box<Sleep>>,
    cancelled: Arc<AtomicBool>,
    _slot: OwnedSemaphorePermit,
}

impl Work {
    /// The deadline counts time spent waiting for a blocking thread too.
    fn start(
        collection: Collection,
        worker: OwnedSemaphorePermit,
        run: impl FnOnce(&[u8], Vec<Arc<ResponseAction>>, &dyn Fn() -> bool) -> Option<Vec<u8>>
        + Send
        + 'static,
    ) -> Self {
        let original = Bytes::from(collection.bytes);
        let input = original.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = cancelled.clone();
        let deadline = Instant::now() + REWRITE_WORK;
        let timer = Box::pin(sleep(REWRITE_WORK));
        let task = tokio::task::spawn_blocking(move || {
            let _worker = worker;
            let within_budget = || !stop.load(Relaxed) && Instant::now() < deadline;
            if !within_budget() {
                return None;
            }
            let changed = run(&input, collection.actions, &within_budget)?;
            within_budget().then_some(changed)
        });
        Self {
            original,
            trailers: collection.trailers,
            task,
            timer,
            cancelled,
            _slot: collection.slot,
        }
    }

    /// A task that fails or misses its deadline leaves the original intact.
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Option<Vec<u8>>> {
        if self.timer.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        Pin::new(&mut self.task)
            .poll(cx)
            .map(|joined| joined.ok().flatten())
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        self.cancelled.store(true, Relaxed);
        // Abort can remove queued work. A running search sees cancellation
        // before its next search and keeps its worker place until it exits.
        self.task.abort();
    }
}

/// Keep intermediate results off the body until every action succeeds.
fn apply_actions(
    input: &[u8],
    actions: Vec<Arc<ResponseAction>>,
    within_budget: &dyn Fn() -> bool,
) -> Option<Vec<u8>> {
    let (first, rest) = actions.split_first()?;
    let mut changed = first.apply(input, within_budget)?;
    for action in rest {
        changed = action.apply(&changed, within_budget)?;
    }
    Some(changed)
}

type Item<E> = Result<Frame<Bytes>, E>;

/// A body either collects, waits for a rewrite, streams the original, or
/// has reached the end. Pending frames are served before reading upstream.
enum State {
    Collecting(Collection),
    Rewriting(Work),
    Streaming,
    Ended,
}

/// The body holds only the frames needed to finish its current state.
struct Rewrite<B: Body> {
    inner: B,
    state: State,
    workers: Arc<Semaphore>,
    /// At most a trailer and the frame that ended a collection early.
    pending: VecDeque<Item<B::Error>>,
}

impl<B: Body<Data = Bytes> + Unpin> Rewrite<B> {
    fn new(
        inner: B,
        actions: Vec<Arc<ResponseAction>>,
        slot: OwnedSemaphorePermit,
        workers: Arc<Semaphore>,
    ) -> Self {
        Self {
            inner,
            state: State::Collecting(Collection {
                bytes: Vec::new(),
                trailers: None,
                timer: Box::pin(sleep(REWRITE_WAIT)),
                actions,
                slot,
            }),
            workers,
            pending: VecDeque::new(),
        }
    }

    /// Yield the body before any trailer or frame that interrupted collection.
    fn finish(
        &mut self,
        bytes: Bytes,
        trailers: Option<Frame<Bytes>>,
        next: Option<Item<B::Error>>,
    ) -> Option<Item<B::Error>> {
        self.pending.extend(trailers.map(Ok));
        self.pending.extend(next);
        if bytes.is_empty() {
            self.pending.pop_front()
        } else {
            Some(Ok(Frame::data(bytes)))
        }
    }

    /// Give up collection without losing the frame that caused it.
    fn stream(
        &mut self,
        cx: &mut Context<'_>,
        next: Option<Item<B::Error>>,
    ) -> Poll<Option<Item<B::Error>>> {
        let State::Collecting(collection) = std::mem::replace(&mut self.state, State::Streaming)
        else {
            unreachable!("streaming after collection")
        };
        match self.finish(Bytes::from(collection.bytes), collection.trailers, next) {
            Some(frame) => Poll::Ready(Some(frame)),
            None => Pin::new(&mut self.inner).poll_frame(cx),
        }
    }

    /// Only a whole body can be rewritten, and only when a worker is free.
    fn rewrite(&mut self, cx: &mut Context<'_>) -> Poll<Option<Item<B::Error>>> {
        let State::Collecting(collection) = std::mem::replace(&mut self.state, State::Ended) else {
            unreachable!("rewriting a collected body")
        };
        let Ok(worker) = self.workers.clone().try_acquire_owned() else {
            return Poll::Ready(self.finish(
                Bytes::from(collection.bytes),
                collection.trailers,
                None,
            ));
        };
        self.state = State::Rewriting(Work::start(collection, worker, apply_actions));
        self.poll_work(cx)
    }

    /// Keep the original available until the blocking job returns a full result.
    fn poll_work(&mut self, cx: &mut Context<'_>) -> Poll<Option<Item<B::Error>>> {
        let State::Rewriting(work) = &mut self.state else {
            unreachable!("polling rewrite work")
        };
        let changed = ready!(work.poll(cx));
        let bytes = changed.map_or_else(|| std::mem::take(&mut work.original), Bytes::from);
        let trailers = work.trailers.take();
        self.state = State::Ended;
        Poll::Ready(self.finish(bytes, trailers, None))
    }

    fn poll_collection(&mut self, cx: &mut Context<'_>) -> Poll<Option<Item<B::Error>>> {
        let State::Collecting(collection) = &mut self.state else {
            unreachable!("collecting a body")
        };
        // An always-ready series of tiny frames must still yield to other tasks.
        for _ in 0..32 {
            if collection.timer.as_mut().poll(cx).is_ready() {
                return self.stream(cx, None);
            }
            match Pin::new(&mut self.inner).poll_frame(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return self.rewrite(cx),
                Poll::Ready(Some(Ok(frame))) if collection.trailers.is_none() => {
                    match frame.into_data() {
                        Ok(data) if data.len() <= MAX_REWRITE_BODY - collection.bytes.len() => {
                            collection.bytes.extend_from_slice(&data);
                        }
                        Ok(data) => return self.stream(cx, Some(Ok(Frame::data(data)))),
                        Err(trailers) => collection.trailers = Some(trailers),
                    }
                }
                Poll::Ready(Some(frame)) => return self.stream(cx, Some(frame)),
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

impl<B: Body<Data = Bytes> + Unpin> Body for Rewrite<B>
where
    B::Error: Unpin,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Item<Self::Error>>> {
        let this = self.get_mut();
        if let Some(frame) = this.pending.pop_front() {
            return Poll::Ready(Some(frame));
        }
        match this.state {
            State::Collecting(_) => this.poll_collection(cx),
            State::Rewriting(_) => this.poll_work(cx),
            State::Streaming => Pin::new(&mut this.inner).poll_frame(cx),
            State::Ended => Poll::Ready(None),
        }
    }
}

#[cfg(test)]
mod tests;
