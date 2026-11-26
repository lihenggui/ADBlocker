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

import android.graphics.Color
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.animation.AnimatedContent
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import androidx.hilt.lifecycle.viewmodel.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dagger.hilt.android.AndroidEntryPoint
import li.mercury.adblocker.core.designsystem.motion.rememberScreenTransitions
import li.mercury.adblocker.core.designsystem.theme.ADBlockerTheme
import li.mercury.adblocker.core.model.ThemeMode
import li.mercury.adblocker.feature.onboarding.impl.OnboardingScreen
import li.mercury.adblocker.ui.ADBlockerApp

@AndroidEntryPoint
class MainActivity : ComponentActivity() {
    private val viewModel: MainActivityViewModel by viewModels()

    override fun onStart() {
        super.onStart()
        viewModel.applyPendingSetup()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        // The splash screen stays until the settings are read, so that the
        // first frame has the right colors and the right screen.
        installSplashScreen().setKeepOnScreenCondition { viewModel.userData.value == null }
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            // Collection begins at STARTED, while the splash is still holding the first draw.
            val userData by viewModel.userData.collectAsStateWithLifecycle()
            val settings = userData ?: return@setContent
            val dark = when (settings.themeMode) {
                ThemeMode.System -> isSystemInDarkTheme()
                ThemeMode.Light -> false
                ThemeMode.Dark -> true
            }
            DisposableEffect(dark) {
                enableEdgeToEdge(
                    statusBarStyle = SystemBarStyle.auto(Color.TRANSPARENT, Color.TRANSPARENT) { dark },
                    navigationBarStyle = SystemBarStyle.auto(LIGHT_NAVIGATION_SCRIM, DARK_NAVIGATION_SCRIM) { dark },
                )
                onDispose {}
            }
            ADBlockerTheme(darkTheme = dark, useDynamicColor = settings.useDynamicColor) {
                // Onboarding is not a destination: there is nothing to go back
                // to from it, and nothing to return to it for. Leaving it is
                // a move between top-level places: one fades out, then the other in.
                val transitions = rememberScreenTransitions()
                AnimatedContent(
                    targetState = settings.onboardingDone,
                    transitionSpec = { transitions.topLevel },
                    label = "onboarding",
                ) { done ->
                    if (done) {
                        ADBlockerApp(listPaneWide = settings.listPaneWide, onListPaneWide = viewModel::setListPaneWide)
                    } else {
                        OnboardingScreen(viewModel = hiltViewModel())
                    }
                }
            }
        }
    }

    private companion object {
        // Match the scrims used by Activity's default edge-to-edge styles.
        val LIGHT_NAVIGATION_SCRIM = Color.argb(230, 255, 255, 255)
        val DARK_NAVIGATION_SCRIM = Color.argb(128, 27, 27, 27)
    }
}
