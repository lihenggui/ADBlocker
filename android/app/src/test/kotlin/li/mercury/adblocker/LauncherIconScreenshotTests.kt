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

package li.mercury.adblocker

import androidx.activity.ComponentActivity
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.screenshottesting.DefaultTestDevices
import li.mercury.adblocker.core.screenshottesting.captureForDevice
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.GraphicsMode
import org.robolectric.annotation.LooperMode

/** The launcher icon's layers as a launcher masks them, and the themed and tile icons. */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@LooperMode(LooperMode.Mode.PAUSED)
class LauncherIconScreenshotTests {
    @get:Rule
    val composeTestRule = createAndroidComposeRule<ComponentActivity>()

    @Test
    fun givenIconLayers_whenMaskedAsALauncherDoes_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone", DefaultTestDevices.PHONE.spec, "LauncherIcon") {
            Row(Modifier.padding(16.dp), horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                // The layers are 108dp; a launcher's mask shows their middle 72dp.
                Box(Modifier.size(72.dp).clip(CircleShape), contentAlignment = Alignment.Center) {
                    Image(painterResource(R.drawable.ic_launcher_background), contentDescription = null, Modifier.requiredSize(108.dp))
                    Image(painterResource(R.drawable.ic_launcher_foreground), contentDescription = null, Modifier.requiredSize(108.dp))
                }
                Box(Modifier.size(72.dp).clip(CircleShape).background(Color(0xFFDCE8DF)), contentAlignment = Alignment.Center) {
                    Image(
                        painterResource(R.drawable.ic_launcher_monochrome),
                        contentDescription = null,
                        Modifier.requiredSize(108.dp),
                        colorFilter = ColorFilter.tint(Color(0xFF1B3A28)),
                    )
                }
                Box(Modifier.size(48.dp).background(Color(0xFF1B3A28), CircleShape).padding(12.dp)) {
                    Image(painterResource(R.drawable.ic_tile), contentDescription = null)
                }
            }
        }
    }
}
