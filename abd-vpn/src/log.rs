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

//! Where the engine says what went wrong. In the app's process nothing
//! reads standard error, so on Android this is logcat, under the tag
//! `abd`; anywhere else, as in the tests, standard error.

#![allow(unsafe_code, reason = "the call into Android's log library")]

/// Says why something failed where the app's developer can read it, and
/// nowhere else. Never anything of what the apps talk to: no host, no
/// address, no app.
pub(crate) fn log(text: &str) {
    #[cfg(target_os = "android")]
    {
        #[link(name = "log")]
        unsafe extern "C" {
            fn __android_log_write(
                priority: std::ffi::c_int,
                tag: *const std::ffi::c_char,
                text: *const std::ffi::c_char,
            ) -> std::ffi::c_int;
        }
        const ERROR: std::ffi::c_int = 6;
        if let Ok(text) = std::ffi::CString::new(text) {
            // SAFETY: both strings end in a zero byte and live through
            // the call, which keeps neither.
            unsafe { __android_log_write(ERROR, c"abd".as_ptr(), text.as_ptr()) };
        }
    }
    #[cfg(not(target_os = "android"))]
    eprintln!("abd: {text}");
}
