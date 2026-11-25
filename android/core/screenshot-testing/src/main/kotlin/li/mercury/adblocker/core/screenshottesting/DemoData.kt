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

package li.mercury.adblocker.core.screenshottesting

import li.mercury.adblocker.core.model.AppCount
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.BlockedCount
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.DayCount
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.FilterList
import li.mercury.adblocker.core.model.FilteredApp
import li.mercury.adblocker.core.model.PassReason
import li.mercury.adblocker.core.model.PinnedHost
import li.mercury.adblocker.core.model.RequestEvent
import li.mercury.adblocker.core.model.StatsSummary
import li.mercury.adblocker.core.model.Verdict
import java.time.Instant
import java.time.LocalDate
import java.time.LocalTime

/**
 * What the demo device filters and has filtered, for the screenshots: a
 * plausible day, with timestamps relative to the clock used to render it.
 */
object DemoData {
    private val today = LocalDate.now(demoClock)
    private val latestRequest = today.atTime(LocalTime.of(14, 59, 41)).atZone(demoClock.zone).toInstant()

    private fun ago(minutes: Long): Instant = demoClock.instant().minusSeconds(minutes * 60)

    val filtered: Set<DemoApp> = setOf(DemoApp.Browser, DemoApp.News, DemoApp.Puzzle, DemoApp.Video, DemoApp.Bank)

    /** Every demo app but the system one, as the app list shows them by default. */
    val entries: List<AppEntry>
        get() = DemoApp.entries.filter { !it.isSystem }.map { app ->
            AppEntry(
                app.installedApp,
                if (app in filtered) {
                    FilteredApp(app.packageName, if (app == DemoApp.Bank) FilterLevel.Hosts else FilterLevel.Full, pausedUntil = null)
                } else {
                    null
                },
            )
        }

    fun entry(app: DemoApp): AppEntry = entries.first { it.packageName == app.packageName }

    val status: DaemonStatus
        get() = DaemonStatus(
            running = true,
            pid = 4321,
            startedAt = ago(60 * 26),
            version = "v0.1.0",
            apps = filtered.size,
            rules = 141_207,
            hostRules = 52_118,
            exclusions = 3_904,
            paused = false,
            resumeAt = null,
            ipv6 = true,
            listsUpdated = ago(60 * 5),
            urlFiltering = true,
            cosmetic = true,
        )

    private val week = listOf(812L, 1_204L, 967L, 1_530L, 1_102L, 1_388L, 1_046L)

    private val counts: Map<DemoApp, Pair<Long, List<Long>>> = mapOf(
        DemoApp.News to (512L to listOf(420L, 610L, 480L, 702L, 530L, 640L, 512L)),
        DemoApp.Browser to (298L to listOf(210L, 330L, 260L, 410L, 300L, 380L, 298L)),
        DemoApp.Puzzle to (176L to listOf(140L, 200L, 170L, 290L, 210L, 250L, 176L)),
        DemoApp.Video to (60L to listOf(42L, 64L, 57L, 128L, 62L, 118L, 60L)),
        DemoApp.Bank to (0L to List(7) { 0L }),
    )

    val appCounts: Map<String, AppCount>
        get() = counts.map { (app, value) ->
            val (today, days) = value
            app.packageName to AppCount(
                today = BlockedCount(requests = today * 3 / 4, hosts = today / 4),
                week = BlockedCount(requests = days.sum() * 3 / 4, hosts = days.sum() / 4),
                days = days,
            )
        }.toMap()

    val stats: StatsSummary
        get() {
            return StatsSummary(
                today = BlockedCount(requests = 784, hosts = 262),
                week = week.mapIndexed { index, total ->
                    DayCount(today.minusDays(6L - index), BlockedCount(requests = total * 3 / 4, hosts = total / 4))
                },
                perApp = appCounts,
            )
        }

    val topApps: List<Pair<AppEntry, Long>>
        get() = listOf(DemoApp.News to 512L, DemoApp.Browser to 298L, DemoApp.Puzzle to 176L).map { (app, n) -> entry(app) to n }

    const val EASYLIST = "https://easylist.to/easylist/easylist.txt"
    const val EASYPRIVACY = "https://easylist.to/easylist/easyprivacy.txt"
    const val ADGUARD_MOBILE = "https://filters.adtidy.org/android/filters/11.txt"
    const val UBLOCK = "https://ublockorigin.github.io/uAssetsCDN/filters/filters.min.txt"
    const val EASYLIST_CHINA = "https://easylist-downloads.adblockplus.org/easylistchina.txt"

    val lists: List<FilterList>
        get() = listOf(
            FilterList(EASYLIST, enabled = true, title = "EasyList", rules = 71_402, updated = ago(60 * 5), error = null),
            FilterList(ADGUARD_MOBILE, enabled = true, title = "AdGuard Mobile Ads", rules = 12_118, updated = ago(60 * 5), error = null),
            FilterList(UBLOCK, enabled = true, title = "uBlock filters", rules = 23_657, updated = ago(60 * 5), error = null),
            FilterList(EASYPRIVACY, enabled = true, title = "EasyPrivacy", rules = 47_817, updated = ago(60 * 5), error = null),
            FilterList(EASYLIST_CHINA, enabled = false, title = "EasyList China", rules = 18_204, updated = ago(60 * 24 * 3), error = null),
        )

    val userRules: String = """
        ! Mine: what the lists miss
        ||ads.example.net^
        ||track.example.org^${'$'}third-party
        @@||cdn.example.com^${'$'}script
        example.com##.promo-banner
        /banner/*/ad_
    """.trimIndent()

    val trusted: List<String> = listOf("bank.example", "intranet.example.org", "pay.example.com")

    val pinned: List<PinnedHost>
        get() = listOf(
            PinnedHost(DemoApp.Bank.packageName, "api.northbank.example", ago(60 * 24 * 2)),
            PinnedHost(DemoApp.Bank.packageName, "auth.northbank.example", ago(60 * 24 * 2)),
            PinnedHost(DemoApp.Video.packageName, "media.reel.example", ago(60 * 3)),
        )

    /** The live requests, newest first, as the activity page shows them. */
    val requests: List<RequestEvent>
        get() = listOf(
            request(1, 0, DemoApp.News, Verdict.Blocked, "ads.adnet.example", "https://ads.adnet.example/serve?slot=top", "script"),
            request(2, 1, DemoApp.News, Verdict.Allowed, "cdn.dailybrief.example", "https://cdn.dailybrief.example/app.js", "script"),
            request(3, 1, DemoApp.Puzzle, Verdict.HostBlocked, "metrics.trackly.example", null, null),
            request(4, 2, DemoApp.Bank, Verdict.Passed, "api.northbank.example", null, null, PassReason.DomainOnly),
            request(5, 3, DemoApp.Browser, Verdict.Blocked, "pixel.trackly.example", "https://pixel.trackly.example/p.gif", "image"),
            request(6, 4, DemoApp.Browser, Verdict.Allowed, "news.example.org", "https://news.example.org/", "document"),
            request(7, 4, DemoApp.Video, Verdict.Passed, "media.reel.example", null, null, PassReason.Pinned),
            request(8, 6, DemoApp.Video, Verdict.Blocked, "vast.adnet.example", "https://vast.adnet.example/vast.xml", "xmlhttprequest"),
        )

    private fun request(
        id: Long,
        secondsAgo: Long,
        app: DemoApp,
        verdict: Verdict,
        host: String,
        url: String?,
        kind: String?,
        reason: PassReason? = null,
    ) = RequestEvent(
        id = id,
        time = latestRequest.minusSeconds(secondsAgo * 7),
        packageName = app.packageName,
        verdict = verdict,
        host = host,
        url = url,
        page = null,
        kind = kind,
        reason = reason,
    )
}
