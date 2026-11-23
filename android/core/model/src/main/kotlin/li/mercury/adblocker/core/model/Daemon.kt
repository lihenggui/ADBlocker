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

/**
 * What `abd status --json` says. Fields only a running daemon knows are null
 * when it is not running.
 */
data class DaemonStatus(
    val running: Boolean,
    val pid: Int?,
    val startedAt: Instant?,
    /** The module's version name, as in module.prop. */
    val version: String?,
    val apps: Int?,
    val rules: Int?,
    val hostRules: Int?,
    val exclusions: Int?,
    /** Paused for every app: no packet filter rules at all. */
    val paused: Boolean,
    /** When a global pause ends by itself; null for one that lasts until resumed. */
    val resumeAt: Instant?,
    val ipv6: Boolean?,
    val listsUpdated: Instant?,
    /** Whether connections are decrypted at all; null from a module that does not say. */
    val urlFiltering: Boolean? = null,
    val filteringScope: FilteringScope = FilteringScope.Chosen,
    /** Whether elements the lists name are hidden on decrypted pages; null from a module that does not say. */
    val cosmetic: Boolean? = null,
    /**
     * When the module turned itself off because a boot did not finish; null
     * when it did not, or from a module that does not say. It stays off
     * until it is started again.
     */
    val bootGuard: Instant? = null,
    /** App UIDs armed by the root daemon, when it reports them. */
    val filteringApps: Int? = null,
)

enum class FilteringScope { Chosen, All }

data class AppSelections(
    val scope: FilteringScope = FilteringScope.Chosen,
    val chosen: Set<String> = emptySet(),
    val excluded: Set<String> = emptySet(),
)

/** How much of an app's traffic can be filtered. */
enum class FilterLevel {
    /** Decrypted, and every request judged by the full lists. */
    Full,

    /** Never decrypted; only rules that block a whole host apply. */
    Hosts,
}

/** An app chosen for filtering, as `abd app list --json` says. */
data class FilteredApp(
    val packageName: String,
    /** Effective HTTP filtering, or null when the app is unavailable. */
    val httpLevel: FilterLevel?,
    /** When a pause of this app ends; null when it is not paused. */
    val pausedUntil: Instant?,
    /** Effective HTTPS filtering, or null when the app is unavailable. */
    val level: FilterLevel? = httpLevel,
    /** Why HTTPS filtering is limited to domains. */
    val levelReason: LevelReason? = if (level == FilterLevel.Hosts) LevelReason.UrlFilteringOff else null,
    val profileSerial: Long? = null,
    val unavailableReason: AppUnavailableReason? = null,
) {
    val appKey: String get() = AppIdentity(packageName, profileSerial).key
}

/** Why an app is filtered by domain only. */
enum class LevelReason {
    /** URL filtering is turned off for every app. */
    UrlFilteringOff,

    /** Without root: the authority is not in the user store, which the user installs it into. */
    CaNotInstalled,

    /** HTTPS exclusions have not been downloaded yet. */
    ExclusionsPending,

    /** The work profile's trust store does not have the filtering certificate. */
    ProfileTrustUnavailable,

    /** The engine did not give a recognized reason. */
    Unknown,
}

/** A host an app would not accept our certificate for, and since when. */
data class PinnedHost(
    val packageName: String,
    val host: String,
    val since: Instant,
    val profileSerial: Long? = null,
) {
    val appKey: String get() = AppIdentity(packageName, profileSerial).key
}
