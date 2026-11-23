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
import li.mercury.adblocker.core.model.RequestEvent
import li.mercury.adblocker.core.model.RuleCheckResult
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
        val rules = object : RulesRepository by unexpected() {
            override suspend fun blockDomain(host: String) {
                blocked += host
            }
        }
        val viewModel = RequestViewModel(request.id, recorder, apps, rules)
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()

        viewModel.blockHost()
        runCurrent()
        assertEquals(listOf(request.host), blocked)
        assertTrue(viewModel.uiState.value.done)

        val message = viewModel.pendingMessages.first().single()
        assertEquals(R.string.feature_activity_blocked_host, message.message.text)
        assertEquals(message, viewModel.pendingMessages.first().single())
        viewModel.onMessageShown(message.id)
        assertEquals(emptyList(), viewModel.pendingMessages.value)
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
            override suspend fun checkUrl(url: String, page: String?, kind: String?): RuleCheckResult {
                assertEquals(request.url, url)
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
}
