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

package li.mercury.adblocker.ui

import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.Box
import androidx.compose.material3.SnackbarHostState
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import li.mercury.adblocker.core.designsystem.theme.ADBlockerTheme
import li.mercury.adblocker.core.ui.AppSnackbarHost
import li.mercury.adblocker.core.ui.LocalHostsSnackbar
import li.mercury.adblocker.core.ui.LocalMessageScope
import li.mercury.adblocker.core.ui.LocalSnackbarHostState
import li.mercury.adblocker.core.ui.MessageEffect
import li.mercury.adblocker.core.ui.MessageQueue
import li.mercury.adblocker.core.ui.UiMessage
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.GraphicsMode
import org.robolectric.annotation.LooperMode
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@LooperMode(LooperMode.Mode.PAUSED)
class SnackbarLifetimeTest {
    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    private val host = SnackbarHostState()
    private val messages = MessageQueue()
    private val screenPresent = mutableStateOf(true)
    private val text get() = compose.activity.getString(android.R.string.ok)
    private val action get() = compose.activity.getString(android.R.string.cancel)

    @Test
    fun givenAConfirmationFromTheCoveredPane_whenItCloses_thenOnlyTheFrontHostKeepsTheConfirmation() {
        showPanes()
        compose.runOnIdle { messages.show(UiMessage(android.R.string.ok)) }
        compose.onNodeWithText(text).assertExists()
        compose.runOnIdle {
            assertTrue(messages.pending.value.isEmpty())
            screenPresent.value = false
        }
        compose.onNodeWithText(text).assertExists()
    }

    @Test
    fun givenAnActionableSnackbar_whenItsScreenCloses_thenUndoIsDismissed() {
        var actions = 0
        showPanes()
        val message = UiMessage(android.R.string.ok, actionLabel = android.R.string.cancel) { actions++ }
        compose.runOnIdle { messages.show(message) }
        compose.onNodeWithText(action).performClick()
        compose.runOnIdle {
            assertEquals(1, actions)
            messages.show(message)
        }
        compose.onNodeWithText(action).assertExists()
        compose.runOnIdle { screenPresent.value = false }
        compose.runOnIdle {
            assertNull(host.currentSnackbarData)
            assertTrue(messages.pending.value.isEmpty())
            assertEquals(1, actions)
        }
    }

    private fun showPanes() {
        compose.setContent {
            ADBlockerTheme(useDynamicColor = false) {
                CompositionLocalProvider(
                    LocalSnackbarHostState provides host,
                    LocalMessageScope provides rememberCoroutineScope(),
                ) {
                    Box {
                        if (screenPresent.value) {
                            CompositionLocalProvider(LocalHostsSnackbar provides false) {
                                MessageEffect(messages.pending, messages::onMessageShown)
                                AppSnackbarHost()
                            }
                        }
                        AppSnackbarHost()
                    }
                }
            }
        }
    }
}
