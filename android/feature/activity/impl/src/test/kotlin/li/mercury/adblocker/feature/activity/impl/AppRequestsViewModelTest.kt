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
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.testing.MainDispatcherRule
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import li.mercury.adblocker.feature.activity.api.ActivityFilter
import org.junit.Rule
import kotlin.test.Test
import kotlin.test.assertEquals

@OptIn(ExperimentalCoroutinesApi::class)
class AppRequestsViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    @Test
    fun givenAnAppRequestsPane_whenReenteringWithinTheGracePeriod_thenMetadataIsReused() = runTest {
        val recorder = ActivityRecorder(unexpected(), TestUserDataRepository())
        val filter = ActivityFilter()
        val target = "com.example.news"
        var reads = 0
        val apps = object : AppsRepository by unexpected() {
            override suspend fun app(packageName: String) = null.also {
                assertEquals(target, packageName)
                reads++
            }
        }
        val viewModel = AppRequestsViewModel(target, recorder, apps, filter)
        runCurrent()
        assertEquals(0, reads)

        val first = backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        assertEquals(1, reads)
        first.cancel()
        advanceTimeBy(4_999)

        val second = backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        assertEquals(1, reads)
        viewModel.showInActivity()
        assertEquals(target, filter.packageName.value)

        second.cancel()
        advanceTimeBy(5_001)
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        assertEquals(2, reads)
    }
}
