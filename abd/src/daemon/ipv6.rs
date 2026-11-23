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

use std::pin::Pin;
use std::time::Duration;

use abd_engine::mitm::{Event, Ipv6Health};
use tokio::sync::mpsc;
use tokio::time::{Instant, Sleep};

use super::reconcile::Reconciler;
use crate::capture;

/// How long IPv6 is left alone after it was found to lead nowhere.
const V6_RETRY: Duration = Duration::from_secs(600);

pub(super) struct Ipv6 {
    pub(super) on: bool,
    pub(super) suspects: mpsc::UnboundedReceiver<Event>,
    pub(super) probe: mpsc::UnboundedReceiver<bool>,
    pub(super) retry: Pin<Box<Sleep>>,
    probed: mpsc::UnboundedSender<bool>,
    probing: bool,
}

impl Ipv6 {
    pub(super) fn new(suspects: mpsc::UnboundedReceiver<Event>) -> Self {
        let retry = Box::pin(tokio::time::sleep(V6_RETRY));
        let (probed, probe) = mpsc::unbounded_channel();
        Self {
            on: true,
            suspects,
            probe,
            retry,
            probed,
            probing: false,
        }
    }

    pub(super) fn on_suspect(&mut self, health: &Ipv6Health) {
        if !self.on || self.probing {
            return;
        }
        // Only a previously reachable server can distinguish a network
        // failure from an app connecting to addresses that lead nowhere.
        let known = health.last_reached();
        self.probing = true;
        let probed = self.probed.clone();
        tokio::spawn(async move {
            let reachable = match known {
                Some(o) => capture::probe(o.dst(), o.mark(), o.uid()).await.is_ok(),
                None => false,
            };
            let _ = probed.send(reachable);
        });
    }

    pub(super) fn on_probe(
        &mut self,
        health: &Ipv6Health,
        reconcile: &mut Reconciler<'_>,
        reachable: bool,
    ) {
        self.probing = false;
        health.reset();
        if !reachable && self.on {
            eprintln!("IPv6 leads nowhere here; leaving it alone for {V6_RETRY:?}");
            self.on = false;
            reconcile.reapply(self.on);
            self.retry.as_mut().reset(Instant::now() + V6_RETRY);
        }
    }

    pub(super) fn on_retry(&mut self, health: &Ipv6Health, reconcile: &mut Reconciler<'_>) {
        self.on = true;
        // What was said of IPv6 while it was left alone is old.
        while self.suspects.try_recv().is_ok() {}
        health.reset();
        reconcile.reapply(self.on);
    }
}
