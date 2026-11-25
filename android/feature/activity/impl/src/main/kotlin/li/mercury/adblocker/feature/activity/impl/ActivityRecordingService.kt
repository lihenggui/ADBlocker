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

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationChannelCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import dagger.hilt.android.AndroidEntryPoint
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import li.mercury.adblocker.core.data.toLoadFailure
import timber.log.Timber
import javax.inject.Inject

/** The user may switch to another app to generate requests during a recording. */
@AndroidEntryPoint
class ActivityRecordingService : Service() {
    @Inject
    lateinit var recorder: ActivityRecorder

    private val recording by lazy {
        RecordingServiceSession(
            recorder,
            CoroutineScope(SupervisorJob() + Dispatchers.Main),
            ::enterForeground,
            ::stopSelf,
        )
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_START -> recording.start(startId)

            ACTION_STOP -> {
                recorder.cancelStart()
                stopSelf()
            }

            else -> recording.unrecognizedStart(startId)
        }
        return START_NOT_STICKY
    }

    private fun enterForeground() {
        val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE else 0
        ServiceCompat.startForeground(this, NOTIFICATION_ID, notification(), type)
    }

    override fun onDestroy() {
        recording.destroy()
        super.onDestroy()
    }

    private fun notification(): Notification {
        NotificationManagerCompat.from(this).createNotificationChannel(
            NotificationChannelCompat.Builder(CHANNEL, NotificationManagerCompat.IMPORTANCE_LOW)
                .setName(getString(R.string.feature_activity_notification_channel))
                .setShowBadge(false)
                .build(),
        )
        val stop = PendingIntent.getService(this, 0, intent(this, ACTION_STOP), PendingIntent.FLAG_IMMUTABLE)
        val open = packageManager.getLaunchIntentForPackage(packageName)?.let {
            PendingIntent.getActivity(this, NOTIFICATION_ID, it, PendingIntent.FLAG_IMMUTABLE)
        }
        return NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_activity_recording)
            .setContentTitle(getString(R.string.feature_activity_notification_title))
            .setContentIntent(open)
            .setOngoing(true)
            .setSilent(true)
            .setShowWhen(false)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .addAction(0, getString(R.string.feature_activity_stop_recording), stop)
            .build()
    }

    internal companion object {
        private const val CHANNEL = "activity_recording"
        private const val NOTIFICATION_ID = 782
        private const val ACTION_START = "li.mercury.adblocker.activity.START"
        private const val ACTION_STOP = "li.mercury.adblocker.activity.STOP"

        private fun intent(context: Context, action: String) = Intent(context, ActivityRecordingService::class.java).setAction(action)

        fun start(context: Context) {
            ContextCompat.startForegroundService(context, intent(context, ACTION_START))
        }

        fun stop(context: Context) {
            context.stopService(intent(context, ACTION_STOP))
        }
    }
}

/** Owns the service's starts and recording job on the main thread. */
internal class RecordingServiceSession(
    private val recorder: ActivityRecorder,
    private val scope: CoroutineScope,
    private val enterForeground: () -> Unit,
    private val finish: (Int) -> Unit,
) {
    private var latestStartId = 0
    private var job: Job? = null

    fun start(startId: Int) {
        latestStartId = startId
        if (job?.isActive == true) return
        if (recorder.state.value.status != RecordingStatus.Starting) {
            finish(startId)
            return
        }
        try {
            enterForeground()
        } catch (e: RuntimeException) {
            Timber.w("recording service: %s", e.javaClass.simpleName)
            recorder.startFailed(e.toLoadFailure())
            finish(startId)
            return
        }
        job = scope.launch {
            recorder.record()
            // Android only stops the service for its most recent start ID.
            finish(latestStartId)
        }
    }

    fun unrecognizedStart(startId: Int) {
        latestStartId = startId
        if (job?.isActive != true) finish(startId)
    }

    fun destroy() {
        if (job?.isCompleted != true) recorder.cancelStart()
        scope.cancel()
    }
}
