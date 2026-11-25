/*
 * Copyright (C) 2026 Mercury Li
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License version 3 as
 * published by the Free Software Foundation.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program. If not, see <https://www.gnu.org/licenses/>.
 */

package li.mercury.adblocker.core.engine

/**
 * The engine in this process, as `libabd_vpn.so` exports it. The contract,
 * with what each function and callback does and when, is docs/engine-jni.md;
 * a change here changes it and the library together.
 */
internal object Engine {
    external fun open(dataDir: String, roots: Array<ByteArray>, localeTags: Array<String>, timeZoneId: String, callbacks: Callbacks): Long

    external fun command(handle: Long, args: Array<String>, stdin: ByteArray?): Result

    external fun stream(handle: Long, args: Array<String>, sink: LineSink): Long

    external fun cancel(handle: Long, id: Long)

    external fun start(handle: Long, tunFd: Int, ipv6: Boolean, dnsServers: Array<String>): Int

    external fun stop(handle: Long)

    external fun close(handle: Long)
}

/** What a command ended with: `abd`'s exit status and output. */
class Result(val exit: Int, val stdout: ByteArray, val stderr: ByteArray)

/** Called from engine threads. */
interface Callbacks {
    /** `VpnService.protect` for an upstream socket. */
    fun protect(fd: Int): Boolean

    /** The uid that owns a connection; -1 if unknown. */
    fun ownerUid(protocol: Int, local: ByteArray, localPort: Int, remote: ByteArray, remotePort: Int): Int

    /** The uid of [packageName]; -1 if it is not installed. */
    fun uidOf(packageName: String): Int

    /** Personal UID and stable app identity per line, separated by one space. */
    fun personalAppIdentities(exclusions: String): String

    /** Whether the authority's certificate is in the user store. */
    fun userCaInstalled(): Boolean

    /** The stack ended by itself, with [code]. */
    fun stopped(code: Int)
}

/** Lines of a stream, from a native thread. */
interface LineSink {
    fun line(text: String)

    fun end(exit: Int)
}
