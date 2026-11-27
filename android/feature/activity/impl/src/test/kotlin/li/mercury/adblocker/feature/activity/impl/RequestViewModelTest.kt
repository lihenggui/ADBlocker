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

package li.mercury.adblocker.feature.activity.impl

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.repository.ActivityBuffer
import li.mercury.adblocker.core.data.repository.ActivityFeed
import li.mercury.adblocker.core.data.repository.ActivityRepository
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.RulesRepository
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.RequestEvent
import li.mercury.adblocker.core.model.RuleCheckResult
import li.mercury.adblocker.core.model.RuleSource
import li.mercury.adblocker.core.model.Verdict
import li.mercury.adblocker.core.testing.MainDispatcherRule
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import org.junit.Rule
import java.time.Instant
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class RequestViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    @Test
    fun givenAHostWasBlocked_whenTheClosingSheetIsCollectedAgain_thenTheConfirmationIsStillPending() = runTest {
        val request = RequestEvent(1, Instant.EPOCH, "com.example.news", Verdict.Allowed, "ads.example", null, null, null, null)
        val activity = object : ActivityRepository by unexpected() {
            override fun feed(packageName: String?, buffer: ActivityBuffer?) = flowOf(ActivityFeed(listOf(request), 0))
        }
        val recorder = ActivityRecorder(activity, TestUserDataRepository())
        assertTrue(recorder.requestStart())
        recorder.record()
        val apps = object : AppsRepository by unexpected() {
            override suspend fun app(packageName: String) = null
        }
        val blocked = mutableListOf<String>()
        val rules = adding(blocked)
        val messages = ActivityMessages()
        val viewModel = RequestViewModel(request.id, recorder, apps, rules, messages)
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()

        viewModel.blockHost()
        runCurrent()
        assertEquals(listOf("||ads.example^"), blocked)
        assertTrue(viewModel.uiState.value.done)

        val message = viewModel.pendingMessages.first().single()
        assertEquals(R.string.feature_activity_blocked_host_everywhere, message.message.text)
        assertEquals(message, viewModel.pendingMessages.first().single())
        messages.queue.onMessageShown(message.id)
        assertEquals(emptyList(), viewModel.pendingMessages.value)
    }

    @Test
    fun givenAPersonalApp_whenTheHostIsBlockedInIt_thenTheRuleNamesItsPackageAndTheMessageItsLabel() = runTest {
        val request = allowed("com.example.news")
        val blocked = mutableListOf<String>()
        val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, "News"), adding(blocked))
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        assertEquals("com.example.news", viewModel.uiState.value.rulePackage)

        viewModel.blockHostInApp()
        runCurrent()

        assertEquals(listOf("||ads.example^\$app=com.example.news"), blocked)
        assertTrue(viewModel.uiState.value.done)
        val message = viewModel.pendingMessages.value.single().message
        assertEquals(R.string.feature_activity_blocked_host_in_app, message.text)
        assertEquals(listOf<Any>("\u2066ads.example\u2069", "News"), message.args)
    }

    @Test
    fun givenAWorkProfileApp_whenTheHostIsBlockedInIt_thenTheRuleNamesThePackageWithoutTheProfile() = runTest {
        val request = allowed("com.example.news", profileSerial = 12)
        val blocked = mutableListOf<String>()
        val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, null), adding(blocked))
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()

        viewModel.blockHostInApp()
        runCurrent()

        assertEquals(listOf("||ads.example^\$app=com.example.news"), blocked)
        assertEquals(listOf<Any>("\u2066ads.example\u2069", "\u2066com.example.news\u2069"), viewModel.pendingMessages.value.single().message.args)
    }

    @Test
    fun givenASharedUidGroup_whenOpened_thenTheHostCanOnlyBeBlockedInEveryApp() = runTest {
        val request = allowed("@com.example.shared")
        val blocked = mutableListOf<String>()
        val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, null), adding(blocked))
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        assertNull(viewModel.uiState.value.rulePackage)

        viewModel.blockHostInApp()
        runCurrent()

        assertEquals(emptyList(), blocked)
        assertFalse(viewModel.uiState.value.done)
        assertTrue(viewModel.pendingMessages.value.isEmpty())
    }

    @Test
    fun givenABlockedEndpoint_whenAllowActionsAreInvoked_thenNoExceptionIsOfferedOrWritten() = runTest {
        for (endpoint in listOf("1.2.3.4:443", "[2001:db8::1]:443", "2001:db8::1", "[1.2.3.4]", "[fe80::1%25wlan0]")) {
            val request = allowed("com.example.news").copy(host = endpoint, verdict = Verdict.HostBlocked)
            val appended = mutableListOf<String>()
            val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, "News"), adding(appended))

            assertNull(viewModel.uiState.value.allowHost, endpoint)
            viewModel.allowHost()
            viewModel.allowHostInApp()
            runCurrent()

            assertTrue(appended.isEmpty(), endpoint)
            assertTrue(viewModel.pendingMessages.value.isEmpty(), endpoint)
        }
    }

    @Test
    fun givenABlockedNameOrAddress_whenOpened_thenItCanBeAllowed() {
        for ((host, ruleHost) in listOf(
            "cdn.example.com" to "cdn.example.com",
            "CDN.Example.com." to "cdn.example.com",
            "1.2.3.4" to "1.2.3.4",
            "[2001:db8::1]" to "[2001:db8::1]",
            "[2001:DB8::1]" to "[2001:db8::1]",
        )) {
            val request = allowed("com.example.news").copy(host = host, verdict = Verdict.HostBlocked)
            assertEquals(ruleHost, RequestUiState(request = request).allowHost, host)
        }
    }

    @Test
    fun givenAHostWasRefused_whenAllowedInAllApps_thenTheExceptionNamesTheRecordedHost() = runTest {
        val request = allowed("com.example.news").copy(host = "cdn.example.com", verdict = Verdict.HostBlocked)
        val appended = mutableListOf<String>()
        val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, "News"), adding(appended))
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        assertEquals("example.com", viewModel.uiState.value.trustSite)
        assertNull(viewModel.uiState.value.displayedRule)

        viewModel.allowHost()
        runCurrent()

        assertEquals(listOf("@@||cdn.example.com^"), appended)
        assertTrue(viewModel.uiState.value.done)
        val message = viewModel.pendingMessages.value.single().message
        assertEquals(R.string.feature_activity_allowed_host_everywhere, message.text)
        assertEquals(listOf<Any>("\u2066cdn.example.com\u2069"), message.args)
    }

    @Test
    fun givenAWorkAppAndBlockedHost_whenAllowedInThatApp_thenTheExceptionNamesItsPackageAndTheMessageItsLabel() = runTest {
        val request = allowed("com.example.news", profileSerial = 12).copy(host = "cdn.example.com", verdict = Verdict.HostBlocked)
        val appended = mutableListOf<String>()
        val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, "News"), adding(appended))
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()

        viewModel.allowHostInApp()
        runCurrent()

        assertEquals(listOf("@@||cdn.example.com^\$app=com.example.news"), appended)
        assertTrue(viewModel.uiState.value.done)
        val message = viewModel.pendingMessages.value.single().message
        assertEquals(R.string.feature_activity_allowed_host_in_app, message.text)
        assertEquals(listOf<Any>("\u2066cdn.example.com\u2069", "News"), message.args)
    }

    @Test
    fun givenAnAllowedRequest_whenTrusted_thenItsSiteIsAddedToTrustedSites() = runTest {
        val request = allowed("com.example.news").copy(host = "cdn.example.com")
        val trusted = mutableListOf<String>()
        val rules = object : RulesRepository by unexpected() {
            override suspend fun trust(host: String) {
                trusted += host
            }
        }
        val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, null), rules)
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()

        viewModel.trustSite()
        runCurrent()

        assertEquals(listOf("example.com"), trusted)
        assertTrue(viewModel.uiState.value.done)
        assertEquals(R.string.feature_activity_trusted, viewModel.pendingMessages.value.single().message.text)
    }

    private fun allowed(packageName: String, profileSerial: Long? = null) = RequestEvent(1, Instant.EPOCH, packageName, Verdict.Allowed, "ads.example", null, null, null, null, profileSerial)

    private suspend fun recorderOf(request: RequestEvent): ActivityRecorder {
        val activity = object : ActivityRepository by unexpected() {
            override fun feed(packageName: String?, buffer: ActivityBuffer?) = flowOf(ActivityFeed(listOf(request), 0))
        }
        val recorder = ActivityRecorder(activity, TestUserDataRepository())
        assertTrue(recorder.requestStart())
        recorder.record()
        return recorder
    }

    private fun appsNaming(request: RequestEvent, label: String?) = object : AppsRepository by unexpected() {
        override suspend fun app(packageName: String) = label?.let {
            assertEquals(request.appKey, packageName)
            AppEntry(InstalledApp(request.packageName, it, icon = null, isSystem = false), filter = null)
        }
    }

    private fun adding(appended: MutableList<String>) = object : RulesRepository by unexpected() {
        override suspend fun checkHost(host: String, app: String) = RuleCheckResult(blocked = false, rule = null, source = null)

        override suspend fun addRule(rule: String): Boolean = true.also { appended += rule }
    }

    @Test
    fun givenARecordingWasReplaced_whenAnOldRequestIsOpened_thenItIsGoneAndOffersNoActions() = runTest {
        val previous = RequestEvent(1, Instant.EPOCH, "com.example.news", Verdict.Allowed, "ads.example", null, null, null, null)
        val current = previous.copy(id = 2, host = "news.example")
        var feed = ActivityFeed(listOf(previous), 0)
        val activity = object : ActivityRepository by unexpected() {
            override fun feed(packageName: String?, buffer: ActivityBuffer?) = flowOf(feed)
        }
        val recorder = ActivityRecorder(activity, TestUserDataRepository())
        assertTrue(recorder.requestStart())
        recorder.record()
        assertEquals(listOf(previous), recorder.state.value.feed.events)
        feed = ActivityFeed(listOf(current), 0)
        assertTrue(recorder.requestStart())
        recorder.record()
        assertEquals(listOf(current), recorder.state.value.feed.events)

        val viewModel = RequestViewModel(previous.id, recorder, unexpected(), unexpected())
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        viewModel.blockHost()
        viewModel.trustSite()
        viewModel.forgetPinned()
        runCurrent()

        val state = viewModel.uiState.value
        assertTrue(state.gone)
        assertNull(state.request)
        assertNull(state.trustSite)
        assertFalse(state.done)
        assertTrue(viewModel.pendingMessages.value.isEmpty())
    }

    @Test
    fun givenARecordedUrl_whenLookupFails_thenTheRequestRemainsWithAnUnavailableRuleMatch() = runTest {
        val request = RequestEvent(1, Instant.EPOCH, "com.example.news", Verdict.Blocked, "ads.example", "https://ads.example/banner", null, null, null)
        val activity = object : ActivityRepository by unexpected() {
            override fun feed(packageName: String?, buffer: ActivityBuffer?) = flowOf(ActivityFeed(listOf(request), 0))
        }
        val recorder = ActivityRecorder(activity, TestUserDataRepository())
        assertTrue(recorder.requestStart())
        recorder.record()
        var appReads = 0
        val apps = object : AppsRepository by unexpected() {
            override suspend fun app(packageName: String) = null.also { appReads++ }
        }
        var fail = true
        val rules = object : RulesRepository by unexpected() {
            override suspend fun checkUrl(url: String, page: String?, kind: String?, app: String?): RuleCheckResult {
                assertEquals(request.url, url)
                assertEquals("com.example.news", app)
                assertNull(page)
                assertNull(kind)
                if (fail) throw AbdException.TimedOut()
                return RuleCheckResult(blocked = false, rule = null, source = null)
            }
        }
        val viewModel = RequestViewModel(request.id, recorder, apps, rules)
        runCurrent()
        assertEquals(0, appReads)
        val shown = backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        val unavailable = viewModel.uiState.value
        assertEquals(request, unavailable.request)
        assertFalse(unavailable.gone)
        assertTrue(unavailable.ruleCheckUnavailable)
        assertNull(unavailable.rule)

        shown.cancel()
        advanceTimeBy(5_001)
        assertEquals(1, appReads)
        fail = false
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        assertEquals(2, appReads)
        assertFalse(viewModel.uiState.value.ruleCheckUnavailable)
        assertEquals(false, viewModel.uiState.value.rule?.blocked)
    }

    @Test
    fun givenEachVerdictAndRuleSource_whenItsDetailsLoad_thenOnlyTheDecidingUserRuleCanBeRemoved() = runTest {
        val list = RuleSource.List("https://list.example/rules.txt")
        val mine = RuleSource.UserRules
        for (verdict in Verdict.entries) {
            for (source in listOf(mine, list, null)) {
                val request = recorded(verdict)
                val exception = "@@||ads.example^"
                val block = "||ads.example^"
                val blocked = verdict == Verdict.Blocked || verdict == Verdict.HostBlocked
                val result = if (blocked) {
                    RuleCheckResult(true, block, source, hasExceptionField = true)
                } else {
                    RuleCheckResult(false, block, mine, exception, source, hasExceptionField = true)
                }
                val rules = EditingRules(result)
                val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, null), rules)
                val collect = backgroundScope.launch { viewModel.uiState.collect {} }
                runCurrent()

                val remove = when {
                    source != mine -> null
                    blocked -> block
                    verdict == Verdict.Allowed -> exception
                    else -> null
                }
                val state = viewModel.uiState.value
                val label = "$verdict from $source"
                assertEquals(remove, state.ruleToRemove, label)
                assertEquals(blocked && remove == null, state.allowHost != null, label)
                assertEquals(blocked, state.allowHostInApp != null, label)
                assertEquals(verdict == Verdict.Allowed && remove == null, state.canBlock, label)
                assertEquals(verdict == Verdict.HostBlocked, rules.checkedHost, label)
                collect.cancel()
            }
        }
    }

    @Test
    fun givenABlockedRecordingNowAllowedByMyRule_whenReopened_thenItOffersRemovalOfThatException() = runTest {
        val request = recorded(Verdict.Blocked)
        val exception = "@@||ads.example^\$app=com.example.news"
        val rules = EditingRules(RuleCheckResult(false, "||ads.example^", null, exception, RuleSource.UserRules, hasExceptionField = true))
        val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, null), rules)
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()

        assertEquals(exception, viewModel.uiState.value.ruleToRemove)
        assertNull(viewModel.uiState.value.allowHost)
        assertNull(viewModel.uiState.value.allowHostInApp)
    }

    @Test
    fun givenAGlobalBlockInMyRules_whenAllowedInOneApp_thenUndoRemovesOnlyTheNewException() = runTest {
        val block = "||ads.example^"
        val exception = "@@||ads.example^\$app=com.example.news"
        val result = RuleCheckResult(true, block, RuleSource.UserRules, hasExceptionField = true)
        for (verdict in listOf(Verdict.Blocked, Verdict.HostBlocked)) {
            val request = recorded(verdict)
            val rules = EditingRules(result)
            val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, "News"), rules)
            val collect = backgroundScope.launch { viewModel.uiState.collect {} }
            runCurrent()

            val state = viewModel.uiState.value
            assertEquals(block, state.ruleToRemove)
            assertEquals(request.host, state.allowHostInApp)
            assertNull(state.allowHost)
            viewModel.allowHost()
            runCurrent()
            assertTrue(rules.edits.isEmpty())

            viewModel.allowHostInApp()
            runCurrent()
            assertEquals(listOf("append" to exception), rules.edits)
            assertTrue(viewModel.uiState.value.done)
            val message = viewModel.pendingMessages.value.single().message
            assertEquals(R.string.feature_activity_allowed_host_in_app, message.text)
            assertEquals(R.string.feature_activity_undo, message.actionLabel)
            checkNotNull(message.onAction).invoke()
            assertEquals(listOf("append" to exception, "remove" to exception), rules.edits)
            collect.cancel()
        }
    }

    @Test
    fun givenAnAppBlockOrARequestWithoutARulePackage_whenBlockedByMyRules_thenOnlyRemovalIsOffered() = runTest {
        val app = "com.example.news"
        val cases = listOf(
            app to "||ads.example^\$app=$app",
            app to "||ads.example^\$script,app=$app",
            "@com.example.shared" to "||ads.example^",
        )
        for ((packageName, block) in cases) {
            val request = recorded(Verdict.Blocked).copy(packageName = packageName)
            val rules = EditingRules(RuleCheckResult(true, block, RuleSource.UserRules, hasExceptionField = true))
            val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, null), rules)
            val collect = backgroundScope.launch { viewModel.uiState.collect {} }
            runCurrent()

            assertEquals(block, viewModel.uiState.value.ruleToRemove)
            assertNull(viewModel.uiState.value.allowHostInApp)
            assertNull(viewModel.uiState.value.allowHost)
            viewModel.allowHostInApp()
            runCurrent()
            assertTrue(rules.edits.isEmpty())
            collect.cancel()
        }
    }

    @Test
    fun givenALocalHostExceptionWithoutADiagnosticMatch_whenOpened_thenRemoveReplacesBothBlockActions() = runTest {
        val request = recorded(Verdict.Allowed).copy(host = "edge.example.co.uk", url = "https://edge.example.co.uk/content")
        val app = "@@||edge.example.co.uk^\$app=com.example.news"
        val global = "@@||edge.example.co.uk^"
        val parent = "@@||example.co.uk^"
        val appParent = "@@||example.co.uk^\$app=com.example.news"
        val cases = listOf(
            "  $app  \r\n" to app,
            global to global,
            parent to parent,
            "$global\n$appParent" to appParent,
        )
        for ((lines, expected) in cases) {
            val rules = EditingRules(userRules = lines)
            val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, null), rules)
            val collect = backgroundScope.launch { viewModel.uiState.collect {} }
            runCurrent()

            val state = viewModel.uiState.value
            assertEquals(expected, state.ruleToRemove)
            assertEquals(expected, state.displayedRule)
            assertEquals(RuleSource.UserRules, state.displayedRuleSource)
            assertFalse(state.canBlock)
            viewModel.blockHost()
            viewModel.blockHostInApp()
            runCurrent()
            assertTrue(rules.edits.isEmpty())

            viewModel.removeRule()
            runCurrent()
            assertEquals(listOf("remove" to expected), rules.edits)
            collect.cancel()
        }
    }

    @Test
    fun givenNoMatchingWholeHostException_whenOpened_thenBlockRemainsAvailable() = runTest {
        val request = recorded(Verdict.Allowed).copy(host = "edge.example.co.uk", url = "https://edge.example.co.uk/content")
        val rules = EditingRules(
            userRules = listOf(
                "! @@||edge.example.co.uk^",
                "@@||edge.example.co.uk^\$image",
                "@@||edge.example.co.uk^\$app=com.example.other",
                "@@||co.uk^",
                "@@||other.co.uk^",
            ).joinToString("\n"),
        )
        val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, null), rules)
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()

        assertEquals(1, rules.userRuleReads)
        assertNull(viewModel.uiState.value.ruleToRemove)
        assertTrue(viewModel.uiState.value.canBlock)
    }

    @Test
    fun givenAnOldCheckReply_whenOpened_thenAllowAndBlockStillWorkWithoutRemove() = runTest {
        val block = "||ads.example^"
        val exception = "@@||ads.example^"
        for (verdict in listOf(Verdict.Blocked, Verdict.Allowed)) {
            val request = recorded(verdict)
            val blocked = verdict == Verdict.Blocked
            val rules = EditingRules(RuleCheckResult(blocked, block, RuleSource.UserRules), changed = false, userRules = exception)
            val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, null), rules)
            val collect = backgroundScope.launch { viewModel.uiState.collect {} }
            runCurrent()

            val state = viewModel.uiState.value
            assertEquals(0, rules.userRuleReads)
            assertNull(state.ruleToRemove)
            assertEquals(blocked, state.allowHost != null)
            assertEquals(!blocked, state.canBlock)
            viewModel.removeRule()
            if (blocked) viewModel.allowHost() else viewModel.blockHost()
            runCurrent()

            assertEquals(listOf("append" to if (blocked) exception else block), rules.edits)
            assertTrue(viewModel.uiState.value.done)
            collect.cancel()
        }
    }

    @Test
    fun givenEachRuleAction_whenUndone_thenItReversesTheExactLine() = runTest {
        val exception = "@@||Ads.Example^\$app=com.Example.News"
        data class Edit(
            val verdict: Verdict,
            val text: Int,
            val line: String,
            val action: RequestViewModel.() -> Unit,
        )
        val cases = listOf(
            Edit(Verdict.Blocked, R.string.feature_activity_allowed_host_in_app, "@@||ads.example^\$app=com.example.news", RequestViewModel::allowHostInApp),
            Edit(Verdict.Blocked, R.string.feature_activity_allowed_host_everywhere, "@@||ads.example^", RequestViewModel::allowHost),
            Edit(Verdict.Allowed, R.string.feature_activity_blocked_host_in_app, "||ads.example^\$app=com.example.news", RequestViewModel::blockHostInApp),
            Edit(Verdict.Allowed, R.string.feature_activity_blocked_host_everywhere, "||ads.example^", RequestViewModel::blockHost),
            Edit(Verdict.Allowed, R.string.feature_activity_rule_removed, exception, RequestViewModel::removeRule),
        )
        for ((verdict, text, line, action) in cases) {
            val request = recorded(verdict)
            val removing = text == R.string.feature_activity_rule_removed
            val result = if (removing) RuleCheckResult(false, "||ads.example^", null, exception, RuleSource.UserRules, hasExceptionField = true) else null
            val rules = EditingRules(result)
            val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, null), rules)
            val collect = backgroundScope.launch { viewModel.uiState.collect {} }
            runCurrent()

            viewModel.action()
            runCurrent()
            assertEquals(listOf((if (removing) "remove" else "append") to line), rules.edits)
            val message = viewModel.pendingMessages.value.single().message
            assertEquals(text, message.text)
            assertEquals(R.string.feature_activity_undo, message.actionLabel)
            assertTrue(viewModel.uiState.value.done)
            checkNotNull(message.onAction).invoke()

            assertEquals(listOf((if (removing) "remove" else "append") to line, (if (removing) "append" else "remove") to line), rules.edits)
            collect.cancel()
        }
    }

    @Test
    fun givenTheRuleWasAlreadyPresent_whenTheAppendChangesNothing_thenUndoCannotRemoveIt() = runTest {
        val request = recorded(Verdict.HostBlocked)
        val rules = EditingRules(changed = false)
        val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, null), rules)
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()

        viewModel.allowHost()
        runCurrent()

        assertEquals(listOf("append" to "@@||ads.example^"), rules.edits)
        assertNull(viewModel.pendingMessages.value.single().message.onAction)
    }

    @Test
    fun givenTheRuleIsOutsideMyRules_whenRemoveChangesNothing_thenTheSheetStaysOpenWithoutUndo() = runTest {
        val request = recorded(Verdict.Blocked)
        val line = "||ads.example^"
        val rules = EditingRules(RuleCheckResult(true, line, RuleSource.UserRules, hasExceptionField = true), changed = false)
        val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, null), rules)
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()

        viewModel.removeRule()
        runCurrent()

        assertEquals(listOf("remove" to line), rules.edits)
        assertFalse(viewModel.uiState.value.done)
        assertEquals(line, viewModel.uiState.value.ruleToRemove)
        val message = viewModel.pendingMessages.value.single().message
        assertEquals(R.string.feature_activity_rule_not_in_my_rules, message.text)
        assertNull(message.actionLabel)
        assertNull(message.onAction)
    }

    private fun recorded(verdict: Verdict) = allowed("com.example.news").copy(
        verdict = verdict,
        url = if (verdict == Verdict.HostBlocked || verdict == Verdict.Passed) null else "https://ads.example/ad",
    )

    private class EditingRules(
        private val result: RuleCheckResult? = null,
        private val changed: Boolean = true,
        private val userRules: String = "",
    ) : RulesRepository by unexpected() {
        val edits = mutableListOf<Pair<String, String>>()
        var checkedHost = false
            private set
        var userRuleReads = 0
            private set

        override suspend fun userRules(): String = userRules.also { userRuleReads++ }

        override suspend fun checkUrl(url: String, page: String?, kind: String?, app: String?) = result ?: noMatch()

        override suspend fun checkHost(host: String, app: String) = (result ?: noMatch()).also { checkedHost = true }

        override suspend fun addRule(rule: String): Boolean = changed.also { edits += "append" to rule }

        override suspend fun removeRule(rule: String): Boolean = changed.also { edits += "remove" to rule }

        private fun noMatch() = RuleCheckResult(blocked = false, rule = null, source = null, hasExceptionField = true)
    }
}
