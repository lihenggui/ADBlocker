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

import androidx.annotation.MainThread
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import li.mercury.adblocker.core.data.repository.ActivityBuffer
import li.mercury.adblocker.core.data.repository.ActivityFeed
import li.mercury.adblocker.core.data.repository.ActivityRepository
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.data.toLoadFailure
import li.mercury.adblocker.core.model.LoadFailure
import timber.log.Timber
import javax.inject.Inject
import javax.inject.Singleton

enum class RecordingStatus { Idle, Starting, Recording, Stopped }

internal data class ActivityRecording(
    val status: RecordingStatus = RecordingStatus.Idle,
    val feed: ActivityFeed = ActivityFeed.Empty,
    val failure: LoadFailure? = null,
)

/** A single recording shared by the service and the screens, kept only in this process. */
@Singleton
class ActivityRecorder @Inject constructor(
    private val activity: ActivityRepository,
    private val userData: UserDataRepository,
) {
    private val recording = MutableStateFlow(ActivityRecording())
    internal val state = recording.asStateFlow()

    @MainThread
    internal fun requestStart(): Boolean {
        val status = recording.value.status
        if (status == RecordingStatus.Starting || status == RecordingStatus.Recording) return false
        recording.update { it.copy(status = RecordingStatus.Starting, failure = null) }
        return true
    }

    @MainThread
    internal fun startFailed(failure: LoadFailure) {
        recording.update { it.copy(status = RecordingStatus.Stopped, failure = failure) }
    }

    @MainThread
    internal fun cancelStart() {
        recording.update { if (it.status == RecordingStatus.Starting) it.copy(status = RecordingStatus.Idle) else it }
    }

    /**
     * Called by the foreground service. Cancel the collection to stop and keep what arrived.
     * A switch between the root module and the VPN ends the recording, so that one
     * recording never mixes the requests of both.
     */
    internal suspend fun record() {
        if (recording.value.status != RecordingStatus.Starting) return
        recording.value = ActivityRecording(status = RecordingStatus.Recording)
        val buffer = ActivityBuffer()
        try {
            val methods = userData.userData.map { it.method }.distinctUntilChanged()
            val started = methods.first()
            coroutineScope {
                val feed = launch { activity.feed(null, buffer).collect { feed -> recording.update { it.copy(feed = feed) } } }
                val switched = launch {
                    methods.first { it != started }
                    feed.cancel()
                }
                feed.join()
                switched.cancel()
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            Timber.w("recording: %s", e.javaClass.simpleName)
            recording.update { it.copy(failure = e.toLoadFailure()) }
        } finally {
            val latest = buffer.snapshot()
            recording.update {
                it.copy(
                    status = RecordingStatus.Stopped,
                    feed = if (latest == ActivityFeed.Empty) it.feed else latest,
                )
            }
        }
    }
}
