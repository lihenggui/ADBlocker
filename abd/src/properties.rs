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

//! Android's system properties.

#![cfg_attr(
    target_os = "android",
    allow(unsafe_code, reason = "read Android system properties")
)]

use std::ffi::CStr;

/// The value of the property `name`, or none when it is empty or unset.
pub(crate) fn get(name: &CStr) -> Option<String> {
    #[cfg(target_os = "android")]
    {
        let mut value = [0; libc::PROP_VALUE_MAX as usize];
        // SAFETY: the property name is NUL-terminated and the output buffer
        // has the capacity required by bionic, including its terminator.
        let len = unsafe { libc::__system_property_get(name.as_ptr(), value.as_mut_ptr().cast()) };
        let value = std::str::from_utf8(value.get(..usize::try_from(len).ok()?)?).ok()?;
        (!value.is_empty()).then(|| value.to_owned())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = name;
        None
    }
}

/// The language tag of the device's system locale. Android keeps the one
/// of the user's locales that the system supports in `persist.sys.locale`,
/// and not the others; before the user chose one, the product's default
/// applies.
pub(crate) fn locale() -> Option<String> {
    get(c"persist.sys.locale").or_else(|| get(c"ro.product.locale"))
}

/// The system time zone ID, if Android has set it.
pub(crate) fn time_zone() -> Option<String> {
    get(c"persist.sys.timezone")
}
