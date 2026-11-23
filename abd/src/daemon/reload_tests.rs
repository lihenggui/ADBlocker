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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::bail;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use super::reconcile::{changed_group_users, same_owners, set_capture};
use super::reload::refresh_filters;
use super::setup::{accept_connections, load_filters};
use crate::paths::Paths;
use crate::watch;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_listeners_accept_fresh_connections_during_a_blocked_control_update() {
    let first = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let second = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let destinations = [first.local_addr().unwrap(), second.local_addr().unwrap()];
    let (dispatch, mut accepted) = mpsc::channel(2);
    let accepting = tokio::spawn(accept_connections(first, second, move |stream| {
        dispatch.try_send(stream).unwrap();
    }));

    let (started, waiting) = tokio::sync::oneshot::channel();
    let (release, locked) = std::sync::mpsc::channel();
    let mut control = tokio::spawn(async move {
        tokio::task::block_in_place(move || {
            started.send(()).unwrap();
            locked.recv_timeout(Duration::from_secs(5))
        })
    });
    waiting.await.unwrap();

    for destination in destinations {
        let client = TcpStream::connect(destination).await.unwrap();
        let handled = tokio::select! {
            stream = accepted.recv() => stream.unwrap(),
            updated = &mut control => panic!("updates finished before accepting: {updated:?}"),
        };
        assert_eq!(handled.peer_addr().unwrap(), client.local_addr().unwrap());
    }
    release.send(()).unwrap();
    control.await.unwrap().unwrap();
    accepting.abort();
    assert!(accepting.await.unwrap_err().is_cancelled());
}

#[test]
fn a_retired_group_does_not_need_a_scoped_package_manager_query() {
    let mut active = BTreeMap::from([
        (10_001, "@com.group".to_owned()),
        (1_010_001, "37:@com.group".to_owned()),
        (1_110_001, "38:com.other".to_owned()),
    ]);
    let changed = BTreeSet::from([10, 11]);
    assert_eq!(changed_group_users(&active, &changed), BTreeSet::from([10]));
    active.remove(&1_010_001);
    assert!(changed_group_users(&active, &changed).is_empty());
}

#[test]
fn live_capture_requires_every_previous_full_app_identity() {
    let active = BTreeMap::from([
        (10_001, "com.app".to_owned()),
        (1_010_001, "33:com.app".to_owned()),
    ]);
    let mut resolved = active.clone();
    resolved.insert(10_002, "com.other".to_owned());
    assert!(same_owners(&active, &resolved));

    resolved.insert(1_010_001, "34:com.app".to_owned());
    assert!(!same_owners(&active, &resolved));

    resolved.remove(&1_010_001);
    assert!(!same_owners(&active, &resolved));
}

#[test]
fn a_failed_capture_update_leaves_no_apps_marked_as_actively_filtered() {
    let active = RwLock::new(Arc::new(BTreeMap::from([(10_001, "com.app".to_owned())])));
    let result = set_capture(&active, |uids| {
        assert_eq!(*uids, BTreeSet::from([10_001]));
        bail!("cannot install capture rules")
    });
    assert!(result.is_err());
    assert!(active.read().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_failed_capture_update_retries_without_another_app_change() {
    let selected = Arc::new(BTreeMap::from([(10_001, "com.app".to_owned())]));
    let active = RwLock::new(selected.clone());
    let mut retry = watch::Retry::new();

    let first = set_capture(&active, |_| Err(anyhow::anyhow!("packet filter busy")));
    retry.arm(first.is_err());
    assert!(active.read().unwrap().is_empty());

    retry.fired().await;
    *active.write().unwrap() = selected;
    let second = set_capture(&active, |uids| {
        assert_eq!(*uids, BTreeSet::from([10_001]));
        Ok(())
    });
    assert!(second.is_ok());
    assert_eq!(active.read().unwrap().len(), 1);
}

#[tokio::test]
async fn only_the_latest_lists_are_published_after_edits_during_a_build() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    std::fs::create_dir(paths.rules()).unwrap();
    let initial = Arc::new(load_filters(&paths, false).unwrap());
    let active = RwLock::new(initial.clone());
    let mut jobs = tokio::task::JoinSet::new();
    let refresh =
        |jobs: &mut _, result| refresh_filters(jobs, &active, &paths, false, result).unwrap();
    std::fs::write(paths.user_rules(), "||first.example^").unwrap();
    refresh(&mut jobs, None);
    std::fs::write(paths.user_rules(), "||second.example^").unwrap();
    refresh(&mut jobs, None);
    assert_eq!(jobs.len(), 1);
    let first = jobs.join_next().await.unwrap().unwrap();
    assert!(!refresh(&mut jobs, Some(first)));
    assert!(Arc::ptr_eq(&initial, &active.read().unwrap()));
    let latest = jobs.join_next().await.unwrap().unwrap();
    assert!(refresh(&mut jobs, Some(latest)));
    assert!(active.read().unwrap().blocks_host("second.example"));
    assert!(!refresh(&mut jobs, None));
    assert!(jobs.is_empty());
}
