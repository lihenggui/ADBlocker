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

package li.mercury.adblocker.feature.filters.impl

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.FilterCatalog
import li.mercury.adblocker.core.data.repository.DataChanges
import li.mercury.adblocker.core.data.repository.ListsRepository
import li.mercury.adblocker.core.data.repository.RulesRepository
import li.mercury.adblocker.core.model.FilterList
import li.mercury.adblocker.core.model.RulesCheck
import li.mercury.adblocker.core.model.UpdateEvent
import li.mercury.adblocker.core.testing.MainDispatcherRule
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import org.junit.Rule
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class FiltersViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    private val noCustomRules = object : RulesRepository by unexpected() {
        override suspend fun userRules() = ""
        override suspend fun check(text: String) = RulesCheck(0, emptyList())
        override suspend fun trustedSites() = emptyList<String>()
    }

    @Test
    fun givenATrustedSite_whenUndoIsCollectedAgain_thenItStillRemovesTrust() = runTest {
        val trusted = mutableSetOf<String>()
        val rules = object : RulesRepository by unexpected() {
            override suspend fun trust(host: String) {
                trusted += host
            }

            override suspend fun untrust(host: String) {
                trusted -= host
            }
        }
        val viewModel = FiltersViewModel(unexpected(), rules, TestUserDataRepository(), DataChanges())
        viewModel.trust("news.example")
        runCurrent()
        assertEquals(setOf("news.example"), trusted)

        val message = viewModel.pendingMessages.first().single()
        assertEquals(R.string.feature_filters_trusted, message.message.text)
        assertEquals(message, viewModel.pendingMessages.first().single())
        viewModel.onMessageShown(message.id)
        assertEquals(emptyList(), viewModel.pendingMessages.value)

        assertEquals(R.string.feature_filters_undo, message.message.actionLabel)
        requireNotNull(message.message.onAction).invoke()
        runCurrent()
        assertTrue(trusted.isEmpty())
    }

    @Test
    fun givenRulesCheckFails_whenLoadingFilters_thenOtherDataAndARecoverableDiagnosticRemain() = runTest {
        val list = FilterList("https://list.example/rules.txt", true, null, 4, null, null)
        val text = "||ads.example^"
        var checkFails = true
        var reads = 0
        val lists = object : ListsRepository by unexpected() {
            override suspend fun lists() = listOf(list).also { reads++ }
        }
        val rules = object : RulesRepository by unexpected() {
            override suspend fun userRules() = text

            override suspend fun check(text: String): RulesCheck {
                if (checkFails) throw AbdException.TimedOut()
                return RulesCheck(1, emptyList())
            }

            override suspend fun trustedSites() = listOf("bank.example")
        }
        val viewModel = FiltersViewModel(lists, rules, TestUserDataRepository(), DataChanges())
        val visible = backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()

        val loaded = viewModel.uiState.value
        assertNull(loaded.failure)
        assertFalse(loaded.loading)
        assertEquals(listOf(list), loaded.lists)
        assertEquals(text, loaded.userRules)
        assertEquals(listOf("bank.example"), loaded.trusted)
        assertNull(loaded.rulesCheck)
        assertTrue(loaded.rulesCheckUnavailable)

        visible.cancel()
        advanceTimeBy(4_999)
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        assertEquals(1, reads)

        checkFails = false
        viewModel.refresh()
        runCurrent()
        assertFalse(viewModel.uiState.value.rulesCheckUnavailable)
        assertEquals(RulesCheck(1, emptyList()), viewModel.uiState.value.rulesCheck)
    }

    @Test
    fun aListUpdateFinishesWhileHiddenAndTheScreenReadsTheResultOnReturn() = runTest {
        val changes = DataChanges()
        val finishUpdate = CompletableDeferred<Unit>()
        var reads = 0
        var updated = false
        val lists = object : ListsRepository by unexpected() {
            override suspend fun lists(): List<FilterList> {
                reads++
                return listOf(FilterList("https://list.example/rules.txt", true, null, if (updated) 4 else null, null, null))
            }

            override fun update() = flow {
                emit(UpdateEvent.Started(1))
                finishUpdate.await()
                updated = true
                changes.notifyChanged()
            }
        }
        val viewModel = FiltersViewModel(lists, noCustomRules, TestUserDataRepository(), changes)
        val visible = backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        assertEquals(1, reads)
        assertNull(viewModel.uiState.value.lists.single().rules)

        viewModel.updateLists()
        runCurrent()
        assertNotNull(viewModel.uiState.value.updating)
        visible.cancel()
        advanceTimeBy(5_000)
        runCurrent()
        finishUpdate.complete(Unit)
        runCurrent()
        assertEquals(1, reads)

        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        assertEquals(2, reads)
        assertEquals(4, viewModel.uiState.value.lists.single().rules)
        assertNull(viewModel.uiState.value.updating)
    }

    @Test
    fun givenAnUndownloadedList_whenEnabled_thenTheSwitchTurnsOnAndOnlyThatListDownloads() = runTest {
        val write = CompletableDeferred<Unit>()
        val download = CompletableDeferred<Unit>()
        var saved = FilterList("https://lists.example/ads.txt", false, null, null, null, null)
        val lists = object : ListsRepository by unexpected() {
            override suspend fun lists() = listOf(saved)

            override suspend fun setEnabled(url: String, enabled: Boolean) {
                write.await()
                saved = saved.copy(enabled = enabled)
            }

            override fun updateList(url: String) = flow {
                assertEquals(saved.url, url)
                emit(UpdateEvent.Started(1))
                download.await()
            }
        }
        val model = FiltersViewModel(lists, noCustomRules, TestUserDataRepository(), DataChanges())
        backgroundScope.launch { model.uiState.collect {} }
        runCurrent()

        model.setEnabled(saved, true)
        model.refresh()
        runCurrent()
        assertTrue(model.uiState.value.lists.single().enabled)
        assertFalse(saved.enabled)

        write.complete(Unit)
        runCurrent()
        assertTrue(model.uiState.value.lists.single().enabled)
        assertNotNull(model.uiState.value.updating)
        assertTrue(model.pendingMessages.value.isEmpty())
        download.complete(Unit)
        runCurrent()
    }

    @Test
    fun givenACachedList_whenEnabled_thenItSkipsDownloadAndFailedDisableRollsBack() = runTest {
        var saved = FilterList("https://lists.example/ads.txt", false, null, 40, null, null)
        val disable = CompletableDeferred<Unit>()
        val lists = object : ListsRepository by unexpected() {
            override suspend fun lists() = listOf(saved)

            override suspend fun setEnabled(url: String, enabled: Boolean) {
                if (!enabled) disable.await()
                saved = saved.copy(enabled = enabled)
            }
        }
        val model = FiltersViewModel(lists, noCustomRules, TestUserDataRepository(), DataChanges())
        backgroundScope.launch { model.uiState.collect {} }
        runCurrent()
        model.setEnabled(saved, true)
        runCurrent()
        assertEquals(40, model.uiState.value.totalRules)
        assertNull(model.uiState.value.updating)
        assertTrue(model.pendingMessages.value.isEmpty())

        model.setEnabled(saved, false)
        runCurrent()
        assertFalse(model.uiState.value.lists.single().enabled)
        disable.completeExceptionally(AbdException.TimedOut())
        runCurrent()
        assertTrue(model.uiState.value.lists.single().enabled)
        assertEquals(R.string.feature_filters_failed, model.pendingMessages.value.single().message.text)
    }

    @Test
    fun givenAnUnsubscribedList_whenAddingItFails_thenThePendingRowGoesBackToTheCatalog() = runTest {
        val entry = FilterCatalog.entries.first()
        val add = CompletableDeferred<Unit>()
        val lists = object : ListsRepository by unexpected() {
            override suspend fun lists() = emptyList<FilterList>()
            override suspend fun add(url: String) = add.await()
        }
        val model = FiltersViewModel(lists, noCustomRules, TestUserDataRepository(), DataChanges())
        backgroundScope.launch { model.uiState.collect {} }
        runCurrent()
        model.subscribe(entry)
        model.refresh()
        runCurrent()
        assertTrue(model.uiState.value.lists.single().enabled)
        assertFalse(entry in model.uiState.value.catalog)

        add.completeExceptionally(AbdException.TimedOut())
        runCurrent()
        assertTrue(model.uiState.value.lists.isEmpty())
        assertTrue(entry in model.uiState.value.catalog)
        assertEquals(R.string.feature_filters_failed, model.pendingMessages.value.single().message.text)
    }
}
