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

package li.mercury.adblocker.core.data.protocol

import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.model.AppUnavailableReason
import li.mercury.adblocker.core.model.BlockedCount
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.LevelReason
import li.mercury.adblocker.core.model.ListUpdateState
import li.mercury.adblocker.core.model.PassReason
import li.mercury.adblocker.core.model.RuleIssueKind
import li.mercury.adblocker.core.model.RuleSource
import li.mercury.adblocker.core.model.UpdateEvent
import li.mercury.adblocker.core.model.Verdict
import li.mercury.adblocker.core.model.WatchEvent
import org.junit.Test
import java.io.File
import java.time.Instant
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * Reads the daemon's outputs in `testdata/protocol/`, which its own tests
 * produce from the same states: what the app takes from each is checked
 * here, so that a change on either side fails a test on the other.
 */
class AbdProtocolTest {
    private val cases = File(System.getProperty("protocolCases") ?: error("protocolCases is set by core/data/build.gradle.kts"))

    private fun case(name: String) = cases.resolve(name).readText()

    @Test
    fun aRunningDaemonsStatusIsRead() {
        val status = parseStatus(case("status.json"))
        assertTrue(status.running)
        assertEquals(4242, status.pid)
        assertEquals(Instant.ofEpochSecond(1763538000), status.startedAt)
        assertEquals(216450, status.rules)
        assertFalse(status.paused)
        assertNull(status.resumeAt)
        assertEquals(true, status.urlFiltering)
        assertEquals(false, status.cosmetic)
        assertNull(status.bootGuard)
    }

    @Test
    fun aModuleTheBootGuardTurnedOffSaysSince() {
        val status = parseStatus(case("status-stopped.json").replace("\"boot_guard\": null", "\"boot_guard\": 1763540000"))
        assertEquals(Instant.ofEpochSecond(1763540000), status.bootGuard)
    }

    @Test
    fun aModuleFromBeforeCosmeticAndTheBootGuardSaysNeither() {
        val status = parseStatus("""{"running": false, "paused": false}""")
        assertNull(status.cosmetic)
        assertNull(status.bootGuard)
    }

    @Test
    fun aStoppedDaemonsStatusSaysWhatTheFilesKnow() {
        val status = parseStatus(case("status-stopped.json"))
        assertFalse(status.running)
        assertNull(status.pid)
        assertNull(status.rules)
        assertTrue(status.paused)
        assertEquals(Instant.ofEpochSecond(1763541600), status.resumeAt)
        assertEquals(5, status.apps)
    }

    @Test
    fun givenSavedScopeAndExclusions_whenParsed_thenChoicesAndExclusionsStaySeparate() {
        val selection = parseAppSelections(case("app-selections.json"))
        assertEquals(FilteringScope.All, selection.scope)
        assertEquals(setOf("com.example.browser", "37:com.example.news"), selection.chosen)
        assertEquals(setOf("@com.example.suite", "com.example.shop"), selection.excluded)
        assertEquals(FilteringScope.Chosen, parseStatus(case("status.json")).filteringScope)
    }

    @Test
    fun filteredAppsCarryTheirEffectiveLevelAndPause() {
        val apps = parseApps(case("apps.json")).associateBy { it.packageName }
        assertEquals(FilterLevel.Full, apps.getValue("com.example.browser").httpLevel)
        assertEquals(Instant.ofEpochSecond(1763541600), apps.getValue("com.example.news").pausedUntil)
        assertEquals(FilterLevel.Full, apps.getValue("com.example.shop").httpLevel)
    }

    @Test
    fun givenOnePackageInBothProfiles_whenRead_thenSelectionsAndTrustAreIndependent() {
        val entries = parseApps(case("apps-work.json"))
        val apps = entries.associateBy { it.appKey }
        assertEquals(2, entries.size)
        assertEquals(setOf("com.example.news", "37:com.example.news"), apps.keys)

        val personal = apps.getValue("com.example.news")
        assertNull(personal.profileSerial)
        assertNull(personal.pausedUntil)
        assertEquals(FilterLevel.Full, personal.level)

        val work = apps.getValue("37:com.example.news")
        assertEquals(personal.packageName, work.packageName)
        assertEquals(37L, work.profileSerial)
        assertEquals(FilterLevel.Full, work.httpLevel)
        assertEquals(FilterLevel.Hosts, work.level)
        assertEquals(LevelReason.ProfileTrustUnavailable, work.levelReason)
        assertEquals(Instant.ofEpochSecond(1763541600), work.pausedUntil)
    }

    @Test
    fun givenWorkEventsAndPins_whenRead_thenTheAppIdentityIncludesTheProfile() {
        val line = """{"time":0,"app":"37:com.example.news","verdict":"passed","host":"example.com","reason":"hosts_only"}"""
        val event = assertIs<WatchEvent.Request>(parseWatchLine(line, 1)).event
        assertEquals("com.example.news", event.packageName)
        assertEquals("37:com.example.news", event.appKey)

        val hosts = parsePinned("""[{"app":"37:com.example.news","host":"example.com","since":1}]""")
        assertEquals("37:com.example.news", hosts.single().appKey)
    }

    @Test
    fun givenWorkInventory_whenRead_thenPackagesAreScopedToTheirSerial() {
        val work = parseWorkProfiles(case("app-profiles.json")).single()
        assertEquals(37L, work.serial)
        assertEquals(setOf("com.example.news", "com.example.work"), work.packages)
        assertEquals(SharedUidGroup("com.example.suite", setOf("com.example.work")), work.groups?.single())
        assertFailsWith<AbdException.BadOutput> { parseWorkProfiles("""{"profiles":[{"serial":0,"packages":[]}]}""") }
    }

    @Test
    fun givenPartialInventory_whenRead_thenUnavailableProfilesDifferFromEmptyProfiles() {
        val profiles = parseWorkProfiles(case("app-profiles-partial.json")).associateBy { it.serial }
        assertEquals(setOf(37L, 38L, 39L), profiles.keys)
        assertEquals(setOf("com.example.news", "com.example.work"), profiles.getValue(37L).packages)
        assertEquals(emptySet<String>(), profiles.getValue(38L).packages)
        assertNull(profiles.getValue(39L).packages)
        assertNull(profiles.getValue(39L).groups)
    }

    @Test
    fun givenRootAppEligibility_whenRead_thenSharedAndUnknownAppsAreDistinct() {
        val eligibility = parseAppEligibility(case("app-eligibility.json"))
        assertEquals(setOf("com.example.eligible", "com.example.shared", "com.example.unknown"), eligibility.apps.keys)
        assertNull(eligibility.apps.getValue("com.example.eligible"))
        assertEquals(AppUnavailableReason.SharedUid, eligibility.apps.getValue("com.example.shared"))
        assertEquals(AppUnavailableReason.Unavailable, eligibility.apps.getValue("com.example.unknown"))
        assertEquals(SharedUidGroup("com.example.suite", setOf("com.example.shared")), eligibility.groups.single())
    }

    @Test
    fun givenWorkGroupEventsAndPins_whenRead_thenBothUseTheGroupTarget() {
        val target = "37:@com.example.suite"
        val line = """{"time":0,"app":"$target","verdict":"passed","host":"example.com","reason":"hosts_only"}"""
        val event = assertIs<WatchEvent.Request>(parseWatchLine(line, 1)).event
        assertEquals("@com.example.suite", event.packageName)
        assertEquals(target, event.appKey)
        val pins = parsePinned("""[{"app":"$target","host":"example.com","since":1}]""")
        assertEquals(target, pins.single().appKey)
    }

    @Test
    fun givenSavedUnavailableApps_whenRead_thenThereIsNoEffectiveFiltering() {
        val apps = parseApps(case("apps-unavailable.json")).associateBy { it.appKey }
        val personal = apps.getValue("com.example.shared")
        val work = apps.getValue("37:com.example.shared")
        assertEquals(AppUnavailableReason.SharedUid, personal.unavailableReason)
        assertEquals(AppUnavailableReason.SharedUid, work.unavailableReason)
        assertEquals(AppUnavailableReason.Unavailable, apps.getValue("com.example.unknown").unavailableReason)
        assertTrue(apps.values.all { it.httpLevel == null && it.level == null && it.levelReason == null })
    }

    @Test
    fun givenRootStatus_whenRead_thenConfiguredAndArmedAppCountsStayDistinct() {
        val status = parseStatus(case("status-filtering-apps.json"))
        assertEquals(5, status.apps)
        assertEquals(3, status.filteringApps)
        assertNull(parseStatus(case("status.json")).filteringApps)
    }

    @Test
    fun filteredAppsSayWhatIsDoneForThemAndWhy() {
        val apps = parseApps(case("apps.json")).associateBy { it.packageName }
        assertEquals(FilterLevel.Full, apps.getValue("com.example.browser").level)
        assertNull(apps.getValue("com.example.browser").levelReason)
        assertEquals(FilterLevel.Full, apps.getValue("com.example.shop").level)
        assertNull(apps.getValue("com.example.shop").levelReason)
    }

    @Test
    fun aLevelReasonThisAppDoesNotKnowIsDomainOnlyForAReasonUnknown() {
        val app = parseApps("""[{"package":"com.example.a","effective":{"http":{"level":"full"},"https":{"level":"hosts","reason":"no_root"}},"paused_until":null}]""").single()
        assertEquals(FilterLevel.Hosts, app.level)
        assertEquals(LevelReason.Unknown, app.levelReason)
        val off = parseApps("""[{"package":"com.example.a","effective":{"http":{"level":"full"},"https":{"level":"hosts","reason":"url_filtering_off"}},"paused_until":null}]""").single()
        assertEquals(LevelReason.UrlFilteringOff, off.levelReason)
        val noCa = parseApps("""[{"package":"com.example.a","effective":{"http":{"level":"full"},"https":{"level":"hosts","reason":"ca_not_installed"}},"paused_until":null}]""").single()
        assertEquals(LevelReason.CaNotInstalled, noCa.levelReason)
    }

    @Test
    fun anAppWithoutEffectiveLevelsIsRejected() {
        assertFailsWith<AbdException.BadOutput> {
            parseApps("""[{"package":"com.example.a","paused_until":null}]""")
        }
    }

    @Test
    fun givenAnOffAppWithoutAnUnavailableReason_whenRead_thenItIsStillUnavailable() {
        val app = parseApps("""[{"package":"com.example.a","effective":{"http":{"level":"off"},"https":{"level":"off"}}}]""").single()
        assertEquals(AppUnavailableReason.Unavailable, app.unavailableReason)
        assertNull(app.httpLevel)
        assertNull(app.level)
    }

    @Test
    fun statsAreSevenDaysOfBlockedCountsWithTheSameDaysPerApp() {
        val stats = parseStats(case("stats.json"))
        val emptyDays = List(5) { BlockedCount.Zero }
        assertEquals(listOf(BlockedCount(4, 0)) + emptyDays + BlockedCount(184, 52), stats.days)
        assertEquals(listOf(BlockedCount(4, 0)) + emptyDays + BlockedCount(184, 43), stats.apps.getValue("com.example.news"))
        assertEquals(List(6) { BlockedCount.Zero } + BlockedCount(0, 9), stats.apps.getValue("com.example.shop"))
    }

    @Test
    fun malformedDailyStatsFailWithoutQuotingPackages() {
        val emptyDays = List(7) { "[0,0]" }
        val valid = emptyDays.joinToString(",", "[", "]")
        val malformed = listOf(
            """{"days":$valid}""",
            """{"days":[],"apps":{}}""",
            """{"days":$valid,"apps":{"com.example.news":[[0,0]]}}""",
        ) + listOf("[0]", "[0,0,0]", "[-1,0]", """["1",0]""", "[true,0]", "[9223372036854775808,0]").map { first ->
            val days = (listOf(first) + emptyDays.drop(1)).joinToString(",", "[", "]")
            """{"days":$valid,"apps":{"com.example.news":$days}}"""
        }
        for (text in malformed) {
            val failure = assertFailsWith<AbdException.BadOutput> { parseStats(text) }
            assertFalse(failure.message.orEmpty().contains("com.example.news"))
        }
    }

    @Test
    fun listsSayWhetherTheyAreOnAndHowTheLastUpdateWent() {
        val lists = parseLists(case("lists.json"))
        assertEquals(3, lists.size)
        assertEquals(72410, lists[0].rules)
        assertEquals("connection timed out", lists[1].error)
        assertFalse(lists[2].enabled)
        assertNull(lists[2].title)
    }

    @Test
    fun theOverviewReadsTheSameStatusStatsAndListsAsTheStandaloneCommands() {
        val overview = parseOverview(case("overview.json"))
        assertEquals(parseStatus(case("status.json")), overview.status)
        assertEquals(parseStats(case("stats.json")), overview.stats)
        assertEquals(parseLists(case("lists.json")), overview.lists)
    }

    @Test
    fun malformedOverviewStatsFailWithoutQuotingPackages() {
        val days = List(STATS_DAYS) { "[0,0]" }.joinToString(",", "[", "]")
        val stats = """{"days":$days,"apps":{"com.example.news":[[0,0]]}}"""
        val failure = assertFailsWith<AbdException.BadOutput> {
            parseOverview("""{"status":{"running":false},"stats":$stats,"lists":[]}""")
        }
        assertTrue(failure.message.orEmpty().startsWith("overview:"))
        assertFalse("com.example.news" in failure.message.orEmpty())
    }

    @Test
    fun anUpdateIsReadEventByEvent() {
        val events = case("update.jsonl").lines().filter { it.isNotBlank() }.map(::parseUpdateEvent)
        assertEquals(UpdateEvent.Started(2), events.first())
        val failed = events.filterIsInstance<UpdateEvent.ListProgress>().last()
        assertEquals(ListUpdateState.Failed, failed.state)
        assertEquals("connection timed out", failed.error)
        assertEquals(UpdateEvent.Finished(updated = 1, unchanged = 0, failed = 1, exclusionsReady = false, exclusionsFailed = 1), events.last())
    }

    @Test
    fun aCompletedUpdateRequiresBothTypedExclusionFields() {
        val done = """"event":"done","updated":1,"unchanged":0,"failed":0"""
        assertEquals(
            UpdateEvent.Finished(updated = 1, unchanged = 0, failed = 0, exclusionsReady = true, exclusionsFailed = 0),
            parseUpdateEvent("""{$done,"exclusions_ready":true,"exclusions_failed":0}"""),
        )
        for (fields in listOf(
            """"exclusions_ready":true""",
            """"exclusions_failed":0""",
            """"exclusions_ready":"true","exclusions_failed":0""",
            """"exclusions_ready":TRUE,"exclusions_failed":0""",
            """"exclusions_ready":true,"exclusions_failed":"0"""",
            """"exclusions_ready":true,"exclusions_failed":-1""",
            """"exclusions_ready":true,"exclusions_failed":01""",
            """"exclusions_ready":null,"exclusions_failed":0""",
            """"exclusions_ready":false,"exclusions_failed":null""",
        )) {
            assertFailsWith<AbdException.BadOutput>(fields) { parseUpdateEvent("""{$done,$fields}""") }
        }
    }

    @Test
    fun pinnedHostsNameTheirApp() {
        val pinned = parsePinned(case("pinned.json"))
        assertEquals(listOf("api.video.example", "drm.video.example"), pinned.map { it.host })
        assertTrue(pinned.all { it.packageName == "com.example.video" })
    }

    @Test
    fun theAllowListIsHosts() {
        assertEquals(listOf("bank.example", "intranet.example"), parseAllowList(case("allow.json")))
    }

    @Test
    fun aRulesCheckNamesTheLinesThatAreNotApplied() {
        val check = parseRulesCheck(case("rules-check.json"))
        assertEquals(5, check.rules)
        assertEquals(listOf(5 to RuleIssueKind.Cosmetic, 6 to RuleIssueKind.Invalid), check.issues.map { it.line to it.kind })
        // The lines the issues name are those of the input the daemon checked.
        val lines = case("rules.txt").lines()
        check.issues.forEach { assertEquals(it.text, lines[it.line - 1]) }
    }

    @Test
    fun aUrlCheckNamesTheRuleAndItsList() {
        val result = parseCheck(case("check.json"))
        assertTrue(result.blocked)
        assertEquals("||adnet.example^", result.rule)
        assertEquals(RuleSource.List("https://easylist.to/easylist/easylist.txt"), result.source)
    }

    @Test
    fun watchedRequestsAndDropsAreRead() {
        val events = case("watch.jsonl").lines().filter { it.isNotBlank() }.mapIndexed { index, line -> parseWatchLine(line, index.toLong()) }
        val requests = events.filterIsInstance<WatchEvent.Request>().map { it.event }
        assertEquals(listOf(Verdict.Blocked, Verdict.Allowed, Verdict.HostBlocked, Verdict.Passed), requests.map { it.verdict })
        assertEquals(Instant.ofEpochMilli(1763538401123), requests.first().time)
        assertEquals("script", requests.first().kind)
        assertEquals(PassReason.Pinned, requests.last().reason)
        assertEquals(WatchEvent.Dropped(12), events.last())
    }

    @Test
    fun anOutputThatIsNotJsonIsABadOutput() {
        assertIs<AbdException.BadOutput>(assertFailsWith<AbdException> { parseStatus("abd: unknown command") })
    }

    @Test
    fun aBadOutputDoesNotQuoteWhatItCouldNotRead() {
        // A watch line names hosts and addresses: none of it may reach a message.
        val line = """{"time": "soon", "app": "com.example.news", "verdict": "blocked", "host": "ads.example.com", "url": "https://ads.example.com/t?id=1"}"""
        val failure = assertFailsWith<AbdException.BadOutput> { parseWatchLine(line, 1) }
        val message = failure.message.orEmpty()
        assertFalse("ads.example.com" in message, message)
        assertFalse("com.example.news" in message, message)
    }
}
