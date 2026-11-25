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

use std::path::Path;

use adblock::lists::ParseOptions;
use adblock::resources::PermissionMask;

use super::{ResponseRule, javascript};
use crate::list_url::{Trust, trusted_list_file};

/// The privileges granted to the source of a rule.
#[derive(Clone, Copy, Default)]
pub struct SourcePermissions {
    user: bool,
    scripts: bool,
    scriptlets: bool,
    responses: bool,
}

impl SourcePermissions {
    pub const USER: Self = Self {
        user: true,
        scripts: true,
        scriptlets: false,
        responses: true,
    };

    pub(crate) fn for_path(path: &Path) -> Self {
        if path.parent().and_then(Path::file_name) == Some("rules".as_ref()) {
            return Self::USER;
        }
        Self {
            user: false,
            scripts: trusted_list_file(path, Trust::Scripts),
            scriptlets: trusted_list_file(path, Trust::Scriptlets),
            responses: trusted_list_file(path, Trust::ResponseRules),
        }
    }

    pub(super) fn is_user(self) -> bool {
        self.user
    }

    pub(crate) fn scripts(self) -> bool {
        self.scripts
    }

    pub(crate) fn allows_response(self, rule: &ResponseRule<'_>) -> bool {
        self.responses || !rule.requires_trusted_source()
    }

    pub(crate) fn matcher(self) -> ParseOptions {
        let mut permissions = PermissionMask::from_bits(u8::from(self.scriptlets));
        if self.scripts {
            permissions |= javascript::PERMISSION;
        }
        ParseOptions {
            permissions,
            ..ParseOptions::default()
        }
    }
}
