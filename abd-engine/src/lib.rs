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

//! What is done with a connection of a filtered app, whoever brought it:
//! the host it asks for is read, and the connection is refused, handed on
//! untouched or decrypted and its requests filtered (`mitm`). Bringing the
//! connections here, connecting on as the app and having the app trust the
//! authority are for the program that uses this: `mitm::Ctx` is where it
//! says how.

pub mod allow;
pub mod apps;
pub mod ca;
pub mod catalog;
pub mod cli;
pub mod clienthello;
pub mod clock;
pub mod cosmetic;
mod diagnostics;
pub mod dns;
pub mod exclusions;
pub mod files;
pub mod filter;
mod host;
mod jsonpath;
pub mod limits;
mod list_files;
mod list_url;
pub mod live;
pub mod malformed;
mod media_type;
pub mod mitm;
pub mod paths;
pub mod pause;
pub mod pins;
pub mod policy;
pub mod protocol;
mod protocol_types;
mod public_suffix;
pub mod quic;
pub mod relay;
mod response_action;
mod rewrite;
mod rule_text;
pub mod rulecheck;
pub mod runtime;
pub mod settings;
pub mod sources;
pub mod state;
pub mod stats;
pub mod tls;
pub mod update;
