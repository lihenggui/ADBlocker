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

package li.mercury.adblocker.core.ui

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.LocalActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.APP_LIST_PERMISSION

/**
 * Asking for the installed app list where the system withholds it. While
 * [withheld], the system's request is shown once if [askAtOnce], and the list is read again
 * on a grant and whenever the app comes back to the front, such as from its
 * settings. The returned action asks again while the system still would, and
 * opens this app's settings once it would not.
 */
@Composable
fun rememberAppListAccess(withheld: Boolean, onRecheck: () -> Unit, askAtOnce: Boolean = true): () -> Unit {
    val context = LocalContext.current
    val activity = LocalActivity.current
    val recheck by rememberUpdatedState(onRecheck)
    var asked by rememberSaveable { mutableStateOf(false) }
    val request = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        asked = true
        if (granted) recheck()
    }
    LaunchedEffect(withheld) {
        if (withheld && askAtOnce && !asked) request.launch(APP_LIST_PERMISSION)
    }
    // Settings and other apps stop this one; the system's request only pauses it.
    val shown by rememberUpdatedState(withheld)
    var left by remember { mutableStateOf(false) }
    LifecycleEventEffect(Lifecycle.Event.ON_STOP) { left = true }
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) {
        if (left && shown) recheck()
        left = false
    }
    return remember(activity, context) {
        {
            if (!asked || activity?.shouldShowRequestPermissionRationale(APP_LIST_PERMISSION) == true) {
                request.launch(APP_LIST_PERMISSION)
            } else {
                val details = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null))
                try {
                    context.startActivity(details.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                } catch (_: ActivityNotFoundException) {
                    // A system without the details page: check again stays, and the system's own settings remain.
                }
            }
        }
    }
}

/** In the place of the app list while the system withholds it. */
@Composable
fun AppListWithheld(onAllow: () -> Unit, onRecheck: () -> Unit, modifier: Modifier = Modifier) {
    Placeholder(
        icon = AdbIcons.Apps,
        title = stringResource(R.string.core_ui_app_list_withheld),
        text = stringResource(R.string.core_ui_app_list_withheld_text),
        modifier = modifier,
        action = { Button(onClick = onAllow) { Text(stringResource(R.string.core_ui_app_list_allow)) } },
        secondaryAction = { OutlinedButton(onClick = onRecheck) { Text(stringResource(R.string.core_ui_app_list_check_again)) } },
        prominent = true,
    )
}
