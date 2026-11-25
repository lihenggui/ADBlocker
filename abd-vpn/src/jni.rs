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

//! What the app calls, and what is asked of the app: the functions of
//! `li.mercury.adblocker.core.engine.Engine`, as docs/engine-jni.md has
//! them. Everything they do is `session.rs`; here are only the names, the
//! conversions and the calls back.

#![allow(
    unsafe_code,
    reason = "the functions the JVM calls, the handle it holds and the descriptor it gives"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr};
use std::os::fd::{FromRawFd as _, OwnedFd, RawFd};
use std::path::Path;
use std::sync::Arc;

use jni::errors::{Result, ThrowRuntimeExAndDefault};
use jni::objects::{Global, JByteArray, JObject, JObjectArray, JString, JValue};
use jni::sys::{jint, jlong};
use jni::{Env, EnvUnowned, JavaVM, jni_sig, jni_str};

use crate::log::log;
use crate::platform::Platform;
use crate::session::{Session, Sink};

const MAX_RESOLVERS: usize = 4;

/// The app's `Callbacks`, called from whatever thread asks.
struct Android {
    callbacks: Global<JObject<'static>>,
}

impl Android {
    /// Calls back into the app. A call that fails, or throws, is no
    /// answer: taken by each caller for the one that filters less.
    fn ask<T>(&self, call: impl FnOnce(&mut Env, &JObject) -> Result<T>) -> Option<T> {
        let vm = JavaVM::singleton().ok()?;
        vm.attach_current_thread(|env| call(env, &self.callbacks))
            .ok()
    }

    fn stopped(&self, code: i32) {
        self.ask(|env, app| {
            env.call_method(
                app,
                jni_str!("stopped"),
                jni_sig!("(I)V"),
                &[JValue::Int(code)],
            )?
            .v()
        });
    }
}

fn address(ip: IpAddr) -> Vec<u8> {
    match ip {
        IpAddr::V4(ip) => ip.octets().to_vec(),
        IpAddr::V6(ip) => ip.octets().to_vec(),
    }
}

impl Platform for Android {
    fn owner(&self, protocol: i32, app: SocketAddr, server: SocketAddr) -> Option<u32> {
        let uid = self.ask(|env, callbacks| {
            let local = env.byte_array_from_slice(&address(app.ip()))?;
            let remote = env.byte_array_from_slice(&address(server.ip()))?;
            env.call_method(
                callbacks,
                jni_str!("ownerUid"),
                jni_sig!("(I[BI[BI)I"),
                &[
                    JValue::Int(protocol),
                    JValue::Object(local.as_ref()),
                    JValue::Int(app.port().into()),
                    JValue::Object(remote.as_ref()),
                    JValue::Int(server.port().into()),
                ],
            )?
            .i()
        })?;
        u32::try_from(uid).ok()
    }

    fn uid_of(&self, package: &str) -> Option<u32> {
        let uid = self.ask(|env, callbacks| {
            let package = env.new_string(package)?;
            env.call_method(
                callbacks,
                jni_str!("uidOf"),
                jni_sig!("(Ljava/lang/String;)I"),
                &[JValue::Object(package.as_ref())],
            )?
            .i()
        })?;
        u32::try_from(uid).ok()
    }

    fn personal_app_identities(&self, exclusions: &BTreeSet<&str>) -> BTreeMap<u32, String> {
        let text = self
            .ask(|env, callbacks| {
                let excluded =
                    env.new_string(exclusions.iter().copied().collect::<Vec<_>>().join("\n"))?;
                let reply = env
                    .call_method(
                        callbacks,
                        jni_str!("personalAppIdentities"),
                        jni_sig!("(Ljava/lang/String;)Ljava/lang/String;"),
                        &[JValue::Object(excluded.as_ref())],
                    )?
                    .l()?;
                JString::cast_local(env, reply)?.try_to_string(env)
            })
            .unwrap_or_default();
        text.lines()
            .take(10_000)
            .filter_map(|line| {
                let (uid, name) = line.split_once(' ')?;
                Some((uid.parse().ok()?, name.to_owned()))
            })
            .collect()
    }

    fn ca_installed(&self) -> bool {
        self.ask(|env, callbacks| {
            env.call_method(callbacks, jni_str!("userCaInstalled"), jni_sig!("()Z"), &[])?
                .z()
        })
        .unwrap_or(false)
    }

    fn protect(&self, socket: RawFd) -> bool {
        self.ask(|env, callbacks| {
            env.call_method(
                callbacks,
                jni_str!("protect"),
                jni_sig!("(I)Z"),
                &[JValue::Int(socket)],
            )?
            .z()
        })
        .unwrap_or(false)
    }
}

/// The app's `LineSink`.
struct Lines {
    sink: Global<JObject<'static>>,
}

impl Sink for Lines {
    fn line(&mut self, text: &str) {
        let Ok(vm) = JavaVM::singleton() else {
            return;
        };
        let _: Result<()> = vm.attach_current_thread(|env| {
            let text = env.new_string(text)?;
            env.call_method(
                &self.sink,
                jni_str!("line"),
                jni_sig!("(Ljava/lang/String;)V"),
                &[JValue::Object(text.as_ref())],
            )?
            .v()
        });
    }

    fn end(&mut self, exit: i32) {
        let Ok(vm) = JavaVM::singleton() else {
            return;
        };
        let _: Result<()> = vm.attach_current_thread(|env| {
            env.call_method(
                &self.sink,
                jni_str!("end"),
                jni_sig!("(I)V"),
                &[JValue::Int(exit)],
            )?
            .v()
        });
    }
}

/// What the handle the app holds stands for.
struct Opened {
    session: Session,
    android: Arc<Android>,
}

/// The engine behind a handle `open` returned, or none for 0.
fn opened(handle: jlong) -> Option<&'static Opened> {
    // SAFETY: a handle other than 0 is the pointer `open` made of a box,
    // which lives until `close`; the app calls nothing after that, as the
    // contract has it.
    unsafe { (handle as *const Opened).as_ref() }
}

fn strings(env: &mut Env, array: &JObjectArray<JString>) -> Result<Vec<String>> {
    let mut all = Vec::new();
    for i in 0..array.len(env)? {
        all.push(array.get_element(env, i)?.try_to_string(env)?);
    }
    Ok(all)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_li_mercury_adblocker_core_engine_Engine_open<'l>(
    mut env: EnvUnowned<'l>,
    _this: JObject<'l>,
    data_dir: JString<'l>,
    roots: JObjectArray<'l, JByteArray<'l>>,
    locale_tags: JObjectArray<'l, JString<'l>>,
    time_zone: JString<'l>,
    callbacks: JObject<'l>,
) -> jlong {
    env.with_env(|env| -> Result<jlong> {
        let dir = data_dir.try_to_string(env)?;
        // Once per process, as the contract has it: a later open keeps the
        // defaults the commands have already shown.
        if !abd_engine::catalog::set_device_locale(
            strings(env, &locale_tags)?.iter().map(String::as_str),
            &time_zone.try_to_string(env)?,
        ) {
            log("the list defaults were already set");
        }
        let mut certificates = Vec::new();
        for i in 0..roots.len(env)? {
            let root = roots.get_element(env, i)?;
            certificates.push(env.convert_byte_array(&root)?);
        }
        let android = Arc::new(Android {
            callbacks: env.new_global_ref(&callbacks)?,
        });
        match Session::open(Path::new(&dir), &certificates, android.clone()) {
            Ok(session) => {
                let opened = Box::new(Opened { session, android });
                Ok(Box::into_raw(opened) as jlong)
            }
            Err(e) => {
                log(&format!("the engine did not open: {e:#}"));
                Ok(0)
            }
        }
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_li_mercury_adblocker_core_engine_Engine_command<'l>(
    mut env: EnvUnowned<'l>,
    _this: JObject<'l>,
    handle: jlong,
    args: JObjectArray<'l, JString<'l>>,
    stdin: JByteArray<'l>,
) -> JObject<'l> {
    env.with_env(|env| -> Result<JObject<'l>> {
        let args = strings(env, &args)?;
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let stdin = if stdin.is_null() {
            Vec::new()
        } else {
            env.convert_byte_array(&stdin)?
        };
        let output = match opened(handle) {
            Some(opened) => opened.session.command(&args, &stdin),
            None => crate::session::Output {
                exit: 1,
                stdout: Vec::new(),
                stderr: b"the engine is not open\n".to_vec(),
            },
        };
        let stdout = env.byte_array_from_slice(&output.stdout)?;
        let stderr = env.byte_array_from_slice(&output.stderr)?;
        env.new_object(
            jni_str!("li/mercury/adblocker/core/engine/Result"),
            jni_sig!("(I[B[B)V"),
            &[
                JValue::Int(output.exit),
                JValue::Object(stdout.as_ref()),
                JValue::Object(stderr.as_ref()),
            ],
        )
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_li_mercury_adblocker_core_engine_Engine_stream<'l>(
    mut env: EnvUnowned<'l>,
    _this: JObject<'l>,
    handle: jlong,
    args: JObjectArray<'l, JString<'l>>,
    sink: JObject<'l>,
) -> jlong {
    env.with_env(|env| -> Result<jlong> {
        let args = strings(env, &args)?;
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut lines = Lines {
            sink: env.new_global_ref(&sink)?,
        };
        Ok(match opened(handle) {
            Some(opened) => opened.session.stream(&args, Box::new(lines)) as jlong,
            None => {
                lines.end(1);
                0
            }
        })
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_li_mercury_adblocker_core_engine_Engine_cancel<'l>(
    _env: EnvUnowned<'l>,
    _this: JObject<'l>,
    handle: jlong,
    id: jlong,
) {
    if let Some(opened) = opened(handle) {
        opened.session.cancel(id as u64);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_li_mercury_adblocker_core_engine_Engine_start<'l>(
    mut env: EnvUnowned<'l>,
    _this: JObject<'l>,
    handle: jlong,
    tun_fd: jint,
    ipv6: jni::sys::jboolean,
    resolvers: JObjectArray<'l, JString<'l>>,
) -> jint {
    if tun_fd < 0 {
        return 2;
    }
    // SAFETY: the app detached the descriptor for us: it is open, and
    // nobody else closes it.
    let tun = unsafe { OwnedFd::from_raw_fd(tun_fd) };
    let Some(opened) = opened(handle) else {
        return 1;
    };
    let started = env
        .with_env(|env| -> Result<bool> {
            let mut servers = Vec::new();
            for index in 0..resolvers.len(env)?.min(MAX_RESOLVERS) {
                let addr = resolvers.get_element(env, index)?.try_to_string(env)?;
                if let Ok(addr) = addr.parse::<SocketAddr>() {
                    servers.push(addr);
                }
            }
            let android = opened.android.clone();
            match opened.session.start(
                tun,
                ipv6,
                servers,
                Box::new(move |code| android.stopped(code)),
            ) {
                Ok(()) => Ok(true),
                Err(e) => {
                    log(&format!("the VPN did not start: {e:#}"));
                    Ok(false)
                }
            }
        })
        .resolve::<jni::errors::LogErrorAndDefault>();
    jint::from(!started)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_li_mercury_adblocker_core_engine_Engine_stop<'l>(
    _env: EnvUnowned<'l>,
    _this: JObject<'l>,
    handle: jlong,
) {
    if let Some(opened) = opened(handle) {
        opened.session.stop();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_li_mercury_adblocker_core_engine_Engine_close<'l>(
    _env: EnvUnowned<'l>,
    _this: JObject<'l>,
    handle: jlong,
) {
    if handle != 0 {
        // SAFETY: the pointer `open` made of a box, given back once.
        drop(unsafe { Box::from_raw(handle as *mut Opened) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unavailable_jvm_cannot_authorize_vpn_operations() {
        let platform = Android {
            callbacks: Global::null(),
        };
        let local = "192.0.2.2:1234".parse().unwrap();
        let remote = "192.0.2.3:443".parse().unwrap();

        assert_eq!(platform.owner(libc::IPPROTO_TCP, local, remote), None);
        assert_eq!(platform.uid_of("com.example.news"), None);
        assert!(
            platform
                .personal_app_identities(&BTreeSet::new())
                .is_empty()
        );
        assert!(!platform.ca_installed());
        assert!(!platform.protect(-1));
    }
}
