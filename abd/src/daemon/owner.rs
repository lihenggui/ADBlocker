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

//! Ends the daemon of a module the app installed once the app is gone.
//!
//! A module in a root manager is turned off and removed there. One the app
//! installed with root alone (`module/direct.sh`) has the app and nothing
//! else: with the app uninstalled it would go on reading what the chosen
//! apps send, and nobody could see that or end it. Its daemon ends instead,
//! as when asked to stop, so that the supervisor removes the rules and
//! starts no other. The data stays, for the app installed again;
//! `direct.sh` removes the module at the next boot if the app is still gone.

use std::path::Path;
use std::pin::Pin;
use std::time::Duration;

use tokio::time::Sleep;

use crate::android;

/// Where `module/direct.sh` installs the module.
const DIRECT: &str = "/data/adb/adblocker_direct";

/// How long the app has to stay out of the list of packages: Android can
/// publish a list without an app while it replaces the app with an update.
const CONFIRM: Duration = Duration::from_secs(30);

pub(super) struct Owner {
    /// Whether the app is in the list of packages, if the list says. Not
    /// asked for a module that a root manager controls.
    app_listed: Option<Box<dyn FnMut() -> Option<bool>>>,
    /// Runs only while the list lacks the app.
    confirm: Option<Pin<Box<Sleep>>>,
}

impl Owner {
    pub(super) fn new() -> Self {
        let by_app = std::env::current_exe().is_ok_and(|program| installed_by_app(&program));
        Self {
            app_listed: by_app.then(|| -> Box<dyn FnMut() -> Option<bool>> {
                // Off the worker, like the other reads of Android's files.
                Box::new(|| tokio::task::block_in_place(android::companion_installed))
            }),
            confirm: None,
        }
    }

    /// As the daemon starts, and whenever the packages changed.
    pub(super) fn on_packages(&mut self) {
        let Some(app_listed) = &mut self.app_listed else {
            return;
        };
        if app_listed() == Some(false) {
            if self.confirm.is_none() {
                self.confirm = Some(Box::pin(tokio::time::sleep(CONFIRM)));
            }
        } else {
            self.confirm = None;
        }
    }

    /// Completes once the app has stayed out of the list for `CONFIRM`.
    pub(super) async fn gone(&mut self) {
        loop {
            match &mut self.confirm {
                Some(confirm) => confirm.await,
                None => std::future::pending().await,
            }
            self.confirm = None;
            if self.app_listed.as_mut().and_then(|listed| listed()) == Some(false) {
                return;
            }
        }
    }
}

fn installed_by_app(program: &Path) -> bool {
    program.starts_with(DIRECT)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use tokio::time::timeout;

    use super::*;

    const DAY: Duration = Duration::from_secs(86_400);

    fn owner(answer: &Rc<Cell<Option<bool>>>) -> Owner {
        let answer = answer.clone();
        Owner {
            app_listed: Some(Box::new(move || answer.get())),
            confirm: None,
        }
    }

    #[test]
    fn knows_the_module_the_app_installed_by_where_direct_sh_puts_it() {
        let script = include_str!("../../../module/direct.sh");
        assert!(
            script
                .lines()
                .any(|line| line == format!("DIRECT={DIRECT}"))
        );
        assert!(installed_by_app(Path::new(
            "/data/adb/adblocker_direct/bin/abd"
        )));
        assert!(!installed_by_app(Path::new(
            "/data/adb/modules/adblocker/bin/abd"
        )));
        assert!(!installed_by_app(Path::new(
            "/data/adb/adblocker_direct_old/bin/abd"
        )));
    }

    #[tokio::test(start_paused = true)]
    async fn a_module_in_a_root_manager_outlives_the_app() {
        let mut owner = Owner {
            app_listed: None,
            confirm: None,
        };
        owner.on_packages();
        assert!(timeout(DAY, owner.gone()).await.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn ends_once_the_app_has_stayed_out_of_the_list() {
        let listed = Rc::new(Cell::new(Some(false)));
        let mut owner = owner(&listed);
        owner.on_packages();
        let early = CONFIRM - Duration::from_secs(1);
        assert!(timeout(early, owner.gone()).await.is_err());
        // Another change of the packages does not start the wait again.
        owner.on_packages();
        timeout(Duration::from_secs(2), owner.gone()).await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn an_app_being_replaced_ends_nothing() {
        let listed = Rc::new(Cell::new(Some(false)));
        let mut owner = owner(&listed);
        owner.on_packages();
        listed.set(Some(true));
        owner.on_packages();
        assert!(timeout(DAY, owner.gone()).await.is_err());

        // Listed again by the time the wait ends, with no change seen.
        listed.set(Some(false));
        owner.on_packages();
        listed.set(Some(true));
        assert!(timeout(DAY, owner.gone()).await.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn a_list_that_says_nothing_ends_nothing() {
        let listed = Rc::new(Cell::new(None));
        let mut owner = owner(&listed);
        owner.on_packages();
        assert!(timeout(DAY, owner.gone()).await.is_err());

        listed.set(Some(false));
        owner.on_packages();
        listed.set(None);
        assert!(timeout(DAY, owner.gone()).await.is_err());
    }
}
