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

package li.mercury.adblocker.sync.work

import android.annotation.SuppressLint
import androidx.work.BackoffPolicy
import androidx.work.Constraints
import androidx.work.NetworkType
import androidx.work.WorkInfo
import java.time.Duration
import java.util.UUID
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class UpdateSchedulerTest {
    private val period = Duration.ofDays(7)
    private val wifi = Constraints.Builder()
        .setRequiredNetworkType(NetworkType.UNMETERED)
        .setRequiresBatteryNotLow(true)
        .build()

    private fun scheduled(state: WorkInfo.State, constraints: Constraints = wifi, repeat: Duration = period) = WorkInfo(
        id = UUID.randomUUID(),
        state = state,
        tags = setOf(UpdateListsWorker::class.java.name),
        constraints = constraints,
        initialDelayMillis = repeat.toMillis(),
        periodicityInfo = WorkInfo.PeriodicityInfo(repeat.toMillis(), repeat.toMillis()),
        workerClassName = UpdateListsWorker::class.java.name,
    )

    @Test
    fun givenMatchingPeriodicWork_whenSchedulingOnStartup_thenBothEnqueuedAndRunningWorkCanBeReused() {
        assertTrue(scheduled(WorkInfo.State.ENQUEUED).matchesPeriodicUpdate(period, wifi))
        assertTrue(scheduled(WorkInfo.State.RUNNING).matchesPeriodicUpdate(period, wifi))
    }

    @Test
    fun givenCancelledOrDifferentPeriodicWork_whenScheduling_thenItCannotBeReused() {
        assertFalse(scheduled(WorkInfo.State.CANCELLED).matchesPeriodicUpdate(period, wifi))
        assertFalse(scheduled(WorkInfo.State.ENQUEUED, Constraints.NONE).matchesPeriodicUpdate(period, wifi))
        assertFalse(scheduled(WorkInfo.State.ENQUEUED, repeat = Duration.ofDays(1)).matchesPeriodicUpdate(period, wifi))
    }

    @SuppressLint("RestrictedApi")
    @Test
    fun givenEitherOneTimeUpdate_whenBuilt_thenItUsesTheSameExponentialBackoff() {
        val regular = oneTimeUpdate(wifiOnly = true).workSpec
        val root = oneTimeUpdate(wifiOnly = false, rootOnly = true).workSpec

        for (request in listOf(regular, root)) {
            assertEquals(BackoffPolicy.EXPONENTIAL, request.backoffPolicy)
            assertEquals(Duration.ofMinutes(30).toMillis(), request.backoffDelayDuration)
            assertFalse(request.constraints.requiresBatteryNotLow())
        }
        assertEquals(NetworkType.UNMETERED, regular.constraints.requiredNetworkType)
        assertEquals(NetworkType.CONNECTED, root.constraints.requiredNetworkType)
        assertTrue(root.input.getBoolean(ROOT_UPDATE, false))
    }
}
