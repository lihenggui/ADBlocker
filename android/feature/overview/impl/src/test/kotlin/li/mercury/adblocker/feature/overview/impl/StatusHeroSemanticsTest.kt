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

package li.mercury.adblocker.feature.overview.impl

import androidx.activity.ComponentActivity
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.snapshots.Snapshot
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import li.mercury.adblocker.core.designsystem.theme.ADBlockerTheme
import li.mercury.adblocker.core.screenshottesting.DemoClockRule
import li.mercury.adblocker.core.screenshottesting.DemoData
import org.junit.Rule
import org.junit.Test
import org.junit.rules.RuleChain
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.GraphicsMode
import org.robolectric.annotation.LooperMode
import kotlin.test.assertEquals

@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@LooperMode(LooperMode.Mode.PAUSED)
class StatusHeroSemanticsTest {
    private val clock = DemoClockRule()
    private val compose = createAndroidComposeRule<ComponentActivity>()

    @get:Rule
    val rules: RuleChain = RuleChain.outerRule(clock).around(compose)

    private val liveRegion = SemanticsMatcher.keyIsDefined(SemanticsProperties.LiveRegion)

    @Test
    fun givenFiltering_whenPausedMidSwap_thenOnlyTheNewStatusIsAnnounced() {
        val status = mutableStateOf(DemoData.status)
        val actions = HeroActions({}, {}, {}, {}, {})
        compose.setContent {
            ADBlockerTheme { StatusHero(status.value, filteredApps = 3, actions = actions, vpn = null) }
        }
        compose.waitForIdle()
        compose.mainClock.autoAdvance = false
        Snapshot.withMutableSnapshot { status.value = DemoData.status.copy(paused = true) }
        // Part way through the swap, while the old title is still laid out.
        compose.mainClock.advanceTimeBy(SWAP_MIDWAY_MS)
        compose.waitForIdle()

        val announced = compose.onAllNodes(liveRegion).fetchSemanticsNodes()
        assertEquals(1, announced.size)
        assertEquals(listOf("Paused"), announced.single().config[SemanticsProperties.ContentDescription])
        assertEquals(0, compose.onAllNodes(hasText("Filtering")).fetchSemanticsNodes().size)
    }

    private companion object {
        const val SWAP_MIDWAY_MS = 120L
    }
}
