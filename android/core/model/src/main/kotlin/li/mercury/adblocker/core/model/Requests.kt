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

package li.mercury.adblocker.core.model

import java.time.Instant

/** What became of a connection or a request. */
enum class Verdict {
    /** A request inside a decrypted connection, blocked. */
    Blocked,

    /** A request inside a decrypted connection, let through. */
    Allowed,

    /** The whole connection refused for its host. */
    HostBlocked,

    /** Handed on unread. */
    Passed,
}

/** Why a connection was handed on unread. */
enum class PassReason {
    Pinned,
    DomainOnly,
    ExclusionsPending,
    Excluded,
    Trusted,
    Sensitive,
    NoName,
    Untrusted,
    Unverified,

    /** The engine did not give a recognized reason. */
    Other,
}

/** One event of `abd watch --json`. */
data class RequestEvent(
    /** Assigned by the app, increasing, to tell events apart. */
    val id: Long,
    val time: Instant,
    val packageName: String,
    val verdict: Verdict,
    val host: String,
    val url: String?,
    val page: String?,
    /** The kind of resource, as the daemon names it (`script`, `image`…). */
    val kind: String?,
    val reason: PassReason?,
    val profileSerial: Long? = null,
) {
    val appKey: String get() = AppIdentity(packageName, profileSerial).key
    val blocked: Boolean get() = verdict == Verdict.Blocked || verdict == Verdict.HostBlocked
}

sealed interface WatchEvent {
    data class Request(val event: RequestEvent) : WatchEvent

    /** The daemon left out [count] events because the app fell behind. */
    data class Dropped(val count: Long) : WatchEvent
}
