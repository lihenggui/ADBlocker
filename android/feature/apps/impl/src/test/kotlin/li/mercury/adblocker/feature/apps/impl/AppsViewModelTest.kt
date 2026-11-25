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

package li.mercury.adblocker.feature.apps.impl

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.DataChanges
import li.mercury.adblocker.core.data.repository.StatsRepository
import li.mercury.adblocker.core.model.AppCount
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.AppIconSource
import li.mercury.adblocker.core.model.AppInventory
import li.mercury.adblocker.core.model.AppUnavailableReason
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.FilteredApp
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.model.StatsSummary
import li.mercury.adblocker.core.testing.MainDispatcherRule
import li.mercury.adblocker.core.testing.unexpected
import org.junit.Rule
import kotlin.coroutines.CoroutineContext
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertSame
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class AppsViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    private val news = InstalledApp("com.example.news", "News", AppIconSource("com.example.news", "test"), isSystem = false)
    private val changes = DataChanges()

    /**
     * The daemon's apps, in memory: [failing] makes a change fail, [holding]
     * keeps a change from ending and [holdingReads] a read, until completed.
     */
    private inner class Daemon : AppsRepository by unexpected() {
        val filtered = mutableMapOf<String, FilterLevel>()
        var failing = false
        var installed = true
        var workUnavailable = false
        var unavailableReason: AppUnavailableReason? = null
        var listWithheld = false
        var allApps = false

        override suspend fun inventory() = AppInventory(apps(), workUnavailable, listWithheld = listWithheld)
        var holding: CompletableDeferred<Unit>? = null
        var holdingReads: CompletableDeferred<Unit>? = null
        var reads = 0
        var cancelledReads = 0

        override suspend fun apps(): List<AppEntry> {
            reads++
            try {
                holdingReads?.await()
            } catch (e: CancellationException) {
                cancelledReads++
                throw e
            }
            if (!installed) return emptyList()
            val app = news.copy(unavailableReason = unavailableReason)
            val filter = filtered[news.packageName]?.let { FilteredApp(news.packageName, it, null) }
            return listOf(AppEntry(app, filter, inAllAppsScope = allApps, excluded = allApps && filter == null))
        }

        override suspend fun setFiltered(packageName: String, filtered: Boolean) = changes.changing {
            holding?.await()
            if (failing) throw AbdException.CommandFailed("app add failed")
            if (filtered) this.filtered[packageName] = this.filtered[packageName] ?: FilterLevel.Hosts else this.filtered -= packageName
        }
    }

    private val daemon = Daemon()
    private var statsFailure: AbdException? = null
    private val stats = object : StatsRepository {
        override suspend fun summary(): StatsSummary {
            statsFailure?.let { throw it }
            return StatsSummary.Empty
        }
    }

    // Made in the test, once the rule has set the main dispatcher. Counts
    // are read with the apps; without them the list is shown all the same.
    private val viewModel by lazy { AppsViewModel(daemon, stats, main.testDispatcher, changes) }

    private fun TestScope.showApps() = backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { viewModel.uiState.collect {} }
        .also { runCurrent() }

    private fun shown(): AppEntry = viewModel.uiState.value.entries.single()

    private fun takeMessage() = viewModel.pendingMessages.value.single().let {
        viewModel.onMessageShown(it.id)
        it.message
    }

    @Test
    fun givenAnAllAppsExclusionIsPending_whenAConcurrentLoadFinishes_thenTheEntryStillShowsItAsExcluded() = runTest {
        daemon.allApps = true
        daemon.filtered[news.packageName] = FilterLevel.Hosts
        showApps()
        val gate = CompletableDeferred<Unit>()
        daemon.holding = gate

        viewModel.setFiltered(shown(), false)
        runCurrent()
        viewModel.refresh()
        runCurrent()

        assertTrue(shown().inAllAppsScope)
        assertTrue(shown().excluded)
        assertFalse(shown().isFiltered)
        assertEquals(0, viewModel.uiState.value.filteredCount)
        gate.complete(Unit)
        runCurrent()
        assertTrue(shown().excluded)
    }

    @Test
    fun givenAnUnchangedUiState_whenReadAgain_thenItReusesTheFilteredList() {
        val entry = AppEntry(news, FilteredApp(news.packageName, FilterLevel.Hosts, null))
        val state = AppsUiState(entries = listOf(entry))

        assertEquals(listOf(entry), state.shown)
        assertSame(state.shown, state.shown)
        assertEquals(1, state.filteredCount)
    }

    @Test
    fun givenCountsCannotBeRead_whenTheAppsReload_thenUnavailableAndZeroAreDistinct() = runTest {
        daemon.filtered[news.packageName] = FilterLevel.Hosts
        statsFailure = AbdException.TimedOut()
        showApps()

        assertTrue(shown().isFiltered)
        assertEquals(ReadState.Unavailable(LoadFailure.TimedOut), viewModel.uiState.value.counts)

        statsFailure = null
        viewModel.refresh()
        runCurrent()

        assertEquals(ReadState.Available(emptyMap<String, AppCount>()), viewModel.uiState.value.counts)
        assertTrue(shown().isFiltered)
    }

    @Test
    fun givenTheSystemWithholdsTheAppList_whenAllowedAndReadAgain_thenTheListIsShown() = runTest {
        daemon.listWithheld = true
        showApps()
        assertTrue(viewModel.uiState.value.listWithheld)
        assertEquals(null, viewModel.uiState.value.failure)

        daemon.listWithheld = false
        viewModel.refresh()
        runCurrent()

        assertFalse(viewModel.uiState.value.listWithheld)
        assertEquals(news, shown().app)
    }

    @Test
    fun givenIncompleteWorkInventory_whenRetriedSuccessfully_thenPersonalAppsStayAndNoticeClears() = runTest {
        daemon.workUnavailable = true
        showApps()
        assertTrue(viewModel.uiState.value.workInventoryUnavailable)
        assertEquals(news, shown().app)
        assertEquals(listOf(shown()), viewModel.uiState.value.shown)
        assertEquals(null, viewModel.uiState.value.failure)

        daemon.workUnavailable = false
        viewModel.refresh()
        runCurrent()
        assertFalse(viewModel.uiState.value.workInventoryUnavailable)
        assertEquals(news, shown().app)
    }

    @Test
    fun givenASavedSharedUidApp_whenRemoved_thenItStaysOffAndCannotBeAddedAgain() = runTest {
        daemon.filtered[news.packageName] = FilterLevel.Hosts
        daemon.unavailableReason = AppUnavailableReason.SharedUid
        showApps()
        assertTrue(shown().isSelected)
        assertFalse(shown().isFiltered)
        assertEquals(0, viewModel.uiState.value.filteredCount)
        assertEquals(listOf(shown()), viewModel.uiState.value.shown)

        viewModel.setFiltered(shown(), false)
        runCurrent()
        assertFalse(shown().isSelected)
        assertFalse(news.packageName in daemon.filtered)
        val message = takeMessage()
        assertEquals(R.string.feature_apps_selection_removed, message.text)
        assertEquals(null, message.onAction)

        viewModel.setFiltered(shown(), true)
        runCurrent()
        assertFalse(shown().isSelected)
        assertFalse(news.packageName in daemon.filtered)
    }

    @Test
    fun givenAnUnacknowledgedUndo_whenCollectedAgain_thenItIsStillPending() = runTest {
        showApps()
        viewModel.setFiltered(shown(), true)
        runCurrent()

        val undo = viewModel.pendingMessages.first().single()
        assertEquals(R.string.feature_apps_undo, undo.message.actionLabel)
        assertEquals(undo, viewModel.pendingMessages.first().single())
        viewModel.onMessageShown(undo.id)
        assertTrue(viewModel.pendingMessages.value.isEmpty())
    }

    @Test
    fun anUndoThatFailedShowsTheAppAsTheDaemonHasIt() = runTest {
        showApps()
        viewModel.setFiltered(shown(), true)
        runCurrent()
        assertTrue(shown().isFiltered)

        daemon.failing = true
        // The list read after the failure is held, to see what is shown
        // before it: the switch as it was just before the undo.
        val reads = CompletableDeferred<Unit>()
        daemon.holdingReads = reads
        takeMessage().onAction!!.invoke()
        runCurrent()
        assertTrue(shown().isFiltered)
        assertEquals(R.string.feature_apps_failed, takeMessage().text)

        // And read again: the undo did not happen, and the daemon says so.
        reads.complete(Unit)
        testScheduler.advanceUntilIdle()
        assertTrue(news.packageName in daemon.filtered)
        assertTrue(shown().isFiltered)
    }

    @Test
    fun givenAnAppRemovedAfterDisabling_whenUndoIsPressed_thenItIsNotSelectedAgain() = runTest {
        daemon.filtered[news.packageName] = FilterLevel.Hosts
        showApps()
        viewModel.setFiltered(shown(), false)
        runCurrent()
        val undo = takeMessage().onAction!!

        daemon.installed = false
        changes.notifyChanged()
        runCurrent()
        undo()
        runCurrent()

        assertTrue(viewModel.uiState.value.entries.isEmpty())
        assertFalse(news.packageName in daemon.filtered)
    }

    @Test
    fun aSwitchThatFailedGoesBackToWhereItWas() = runTest {
        showApps()
        daemon.failing = true
        viewModel.setFiltered(shown(), true)
        runCurrent()
        assertFalse(shown().isFiltered)
        assertEquals(R.string.feature_apps_failed, takeMessage().text)
    }

    @Test
    fun givenARemovedApp_whenUndoIsPressed_thenTheSelectionIsRestored() = runTest {
        daemon.filtered[news.packageName] = FilterLevel.Full
        showApps()
        viewModel.setFiltered(shown(), false)
        runCurrent()
        assertFalse(shown().isFiltered)

        takeMessage().onAction!!.invoke()
        runCurrent()
        assertTrue(shown().isFiltered)
        assertTrue(news.packageName in daemon.filtered)
    }

    @Test
    fun aListReadWhileTheSwitchIsBeingDoneKeepsTheSwitchWhereItWasTurned() = runTest {
        showApps()
        val hold = CompletableDeferred<Unit>()
        daemon.holding = hold
        viewModel.setFiltered(shown(), true)
        runCurrent()

        assertEquals(FilterLevel.Hosts, shown().filter?.httpLevel)

        // Read before the daemon has the app: it must not turn the switch back.
        viewModel.refresh()
        runCurrent()
        assertTrue(shown().isFiltered)

        hold.complete(Unit)
        testScheduler.advanceUntilIdle()
        assertTrue(shown().isFiltered)
        assertTrue(news.packageName in daemon.filtered)
    }

    @Test
    fun appLabelsAreSortedOnTheBackgroundDispatcher() = runTest {
        val testDispatcher = StandardTestDispatcher(testScheduler)
        var sorting = false
        val backgroundDispatcher = object : CoroutineDispatcher() {
            override fun dispatch(context: CoroutineContext, block: Runnable) {
                testDispatcher.dispatch(context) {
                    sorting = true
                    try {
                        block.run()
                    } finally {
                        sorting = false
                    }
                }
            }
        }
        val books = InstalledApp("com.example.books", "Books", AppIconSource("com.example.books", "test"), isSystem = false)
        val entries = listOf(AppEntry(news, null), AppEntry(books, null))
        val repository = object : AppsRepository by unexpected() {
            override suspend fun inventory() = AppInventory(apps())

            override suspend fun apps(): List<AppEntry> = object : AbstractList<AppEntry>() {
                override val size: Int get() = entries.size

                override fun get(index: Int): AppEntry {
                    assertTrue(sorting)
                    return entries[index]
                }
            }
        }
        val viewModel = AppsViewModel(repository, unexpected(), backgroundDispatcher, changes)
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        assertEquals(listOf("Books", "News"), viewModel.uiState.value.entries.map { it.app.label })
    }

    @Test
    fun readsAreCancelledWhenTheAppListIsHiddenAndResumeOnReturn() = runTest {
        runCurrent()
        changes.notifyChanged()
        runCurrent()
        assertEquals(0, daemon.reads)

        val visible = showApps()
        assertEquals(1, daemon.reads)
        daemon.holdingReads = CompletableDeferred()
        changes.notifyChanged()
        runCurrent()
        assertEquals(2, daemon.reads)

        visible.cancel()
        runCurrent()
        assertEquals(0, daemon.cancelledReads)
        advanceTimeBy(5_000)
        runCurrent()
        assertEquals(1, daemon.cancelledReads)
        changes.notifyChanged()
        viewModel.refresh()
        runCurrent()
        assertEquals(2, daemon.reads)

        daemon.holdingReads = null
        showApps()
        assertEquals(3, daemon.reads)
    }
}
