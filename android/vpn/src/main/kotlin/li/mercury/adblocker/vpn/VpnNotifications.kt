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

package li.mercury.adblocker.vpn

import android.Manifest
import android.app.Notification
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.text.format.DateFormat
import androidx.core.app.NotificationChannelCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import dagger.hilt.android.qualifiers.ApplicationContext
import li.mercury.adblocker.core.model.ReadState
import java.util.Date
import javax.inject.Inject

/**
 * The notifications of the VPN: the ongoing one while it runs, and the one
 * that says another VPN app took it. They say how many apps are filtered,
 * never which.
 */
internal class VpnNotifications @Inject constructor(
    @ApplicationContext private val context: Context,
) {
    fun createChannels() {
        NotificationManagerCompat.from(context).createNotificationChannelsCompat(
            listOf(
                NotificationChannelCompat.Builder(CHANNEL_RUNNING, NotificationManagerCompat.IMPORTANCE_LOW)
                    .setName(context.getString(R.string.vpn_channel_running))
                    .setShowBadge(false)
                    .build(),
                NotificationChannelCompat.Builder(CHANNEL_STOPPED, NotificationManagerCompat.IMPORTANCE_DEFAULT)
                    .setName(context.getString(R.string.vpn_channel_stopped))
                    .build(),
            ),
        )
    }

    /** The ongoing notification; [shown] is null while the VPN starts. */
    fun running(shown: Shown?): Notification {
        val status = shown?.status?.valueOrNull
        val resumeAt = status?.resumeAt
        val title = when {
            shown == null || shown.status is ReadState.Pending -> context.getString(R.string.vpn_starting)

            status == null -> context.getString(R.string.vpn_status_unavailable)

            status.paused && resumeAt != null -> context.getString(
                R.string.vpn_paused_until,
                DateFormat.getTimeFormat(context).format(Date.from(resumeAt)),
            )

            status.paused -> context.getString(R.string.vpn_paused)

            shown.apps == 0 -> context.getString(R.string.vpn_no_apps)

            else -> context.resources.getQuantityString(R.plurals.vpn_filtering, shown.apps, shown.apps)
        }
        val open = openApp()
        val builder = NotificationCompat.Builder(context, CHANNEL_RUNNING)
            .setSmallIcon(R.drawable.ic_vpn_notification)
            .setContentTitle(title)
            .setContentIntent(open)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setShowWhen(false)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
        when {
            shown == null || shown.apps == 0 || status == null -> Unit
            status.paused -> builder.addAction(0, context.getString(R.string.vpn_action_resume), action(VpnActionReceiver.ACTION_RESUME))
            else -> builder.addAction(0, context.getString(R.string.vpn_action_pause_hour), action(VpnActionReceiver.ACTION_PAUSE))
        }
        if (open != null) builder.addAction(0, context.getString(R.string.vpn_action_open), open)
        return builder.build()
    }

    /** Says, once, that another VPN app took the VPN: the chosen apps are no longer filtered. */
    fun postRevoked() {
        // Without the permission the user turned down, there is nothing to post.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        val text = context.getString(R.string.vpn_revoked_text)
        val notification = NotificationCompat.Builder(context, CHANNEL_STOPPED)
            .setSmallIcon(R.drawable.ic_vpn_notification)
            .setContentTitle(context.getString(R.string.vpn_revoked_title))
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setContentIntent(openApp())
            .setAutoCancel(true)
            .setCategory(NotificationCompat.CATEGORY_STATUS)
            .build()
        NotificationManagerCompat.from(context).notify(REVOKED_ID, notification)
    }

    /** The VPN runs again: what said it had stopped no longer holds. */
    fun cancelRevoked() {
        NotificationManagerCompat.from(context).cancel(REVOKED_ID)
    }

    /** Opens the app; also what the system's VPN settings open. */
    fun openApp(): PendingIntent? = context.packageManager.getLaunchIntentForPackage(context.packageName)?.let {
        PendingIntent.getActivity(context, 0, it, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    private fun action(action: String): PendingIntent = PendingIntent.getBroadcast(
        context,
        0,
        Intent(context, VpnActionReceiver::class.java).setAction(action),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

    companion object {
        const val RUNNING_ID = 1
        private const val REVOKED_ID = 2
        private const val CHANNEL_RUNNING = "vpn"
        private const val CHANNEL_STOPPED = "vpn_stopped"
    }
}
