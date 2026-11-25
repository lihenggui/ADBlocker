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

//! Command result types shared by producers and output code.

use serde::Serialize;

/// A line of `abd update --json`.
#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum Update {
    Start {
        lists: usize,
    },
    List {
        url: String,
        state: ListState,
        rules: Option<usize>,
        error: Option<String>,
    },
    Done {
        updated: usize,
        unchanged: usize,
        failed: usize,
        exclusions_ready: bool,
        exclusions_failed: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ListState {
    Fetching,
    Updated,
    Unchanged,
    Failed,
}

#[derive(Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct RulesCheck {
    pub rules: usize,
    pub issues: Vec<Issue>,
}

#[derive(Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct Issue {
    pub line: usize,
    pub text: String,
    pub kind: IssueKind,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    Invalid,
    Unsupported,
    Inapplicable,
    Untrusted,
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub blocked: bool,
    pub rule: Option<String>,
    /// The address of the list the rule is in, `rules` for the user's.
    pub source: Option<String>,
}

/// A line of `abd watch --json`: a request, or a connection.
#[derive(Debug, Serialize)]
pub struct Event {
    /// Milliseconds since the epoch.
    pub time: u64,
    pub app: String,
    pub verdict: Seen,
    pub host: String,
    pub url: Option<String>,
    pub page: Option<String>,
    pub kind: Option<&'static str>,
    pub reason: Option<Reason>,
}

/// What became of a request or a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Seen {
    Blocked,
    Allowed,
    HostBlocked,
    Passed,
}

/// Events left out because the watcher fell behind.
#[derive(Debug, Serialize)]
pub struct Dropped {
    pub dropped: u64,
}

/// Why a connection is handed on unread, as `abd watch` names it.
#[derive(Debug, PartialEq, Eq, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// The app does not accept us for the host.
    Pinned,
    /// URL filtering is off or HTTPS inspection is unavailable.
    #[serde(rename = "hosts_only")]
    DomainOnly,
    /// One or more HTTPS exclusion sources have no usable copy.
    ExclusionsPending,
    /// An exclusion list names the host.
    Excluded,
    /// The user trusts the host.
    Trusted,
    /// The server's certificate says not to decrypt.
    Sensitive,
    /// No host name in the ClientHello.
    NoName,
    /// The app cannot be made to trust us yet.
    Untrusted,
    /// The server could not be verified.
    Unverified,
}
