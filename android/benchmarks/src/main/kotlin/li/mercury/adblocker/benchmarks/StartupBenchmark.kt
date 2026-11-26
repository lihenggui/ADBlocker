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

package li.mercury.adblocker.benchmarks

import androidx.benchmark.macro.BaselineProfileMode
import androidx.benchmark.macro.CompilationMode
import androidx.benchmark.macro.FrameTimingMetric
import androidx.benchmark.macro.MacrobenchmarkScope
import androidx.benchmark.macro.StartupMode
import androidx.benchmark.macro.StartupTimingMetric
import androidx.benchmark.macro.junit4.BaselineProfileRule
import androidx.benchmark.macro.junit4.MacrobenchmarkRule
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.BySelector
import androidx.test.uiautomator.Direction
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.UiObject2
import androidx.test.uiautomator.Until
import org.junit.Before
import org.junit.Rule
import org.junit.Test

private const val PACKAGE = "li.mercury.adblocker"
private const val TIMEOUT_MS = 30_000L
private const val PROFILE_ITERATIONS = 6

class StartupProfile {
    @get:Rule val profile = BaselineProfileRule()

    @Before
    fun prepareApp() = seedReturningUser()

    @Test
    fun returningUserStartup() = profile.collect(
        packageName = PACKAGE,
        maxIterations = PROFILE_ITERATIONS,
        includeInStartupProfile = true,
        filterPredicate = ::isProfileRule,
    ) {
        launchOverview()
    }

    @Test
    fun browseScreens() = profile.collect(
        packageName = PACKAGE,
        maxIterations = PROFILE_ITERATIONS,
        filterPredicate = ::isProfileRule,
    ) {
        launchOverview()
        device.openAppList()
        device.scrollList()
        device.openDestination("Filters")
        device.requireObject(By.text("Update now"))
        device.scrollList()
        device.openDestination("Activity")
        device.requireObject(By.desc("Start recording"))
        device.requireObject(By.desc("Settings")).click()
        device.requireObject(By.text("Settings"))
    }
}

class StartupBenchmark {
    @get:Rule val benchmark = MacrobenchmarkRule()

    @Before
    fun prepareApp() = seedReturningUser()

    @Test
    fun coldStartWithoutProfile() = measure(CompilationMode.None())

    @Test
    fun coldStartWithProfile() = measure(CompilationMode.Partial(BaselineProfileMode.Require))

    @Test
    fun scrollAppList() = benchmark.measureRepeated(
        packageName = PACKAGE,
        metrics = listOf(FrameTimingMetric()),
        iterations = 5,
        compilationMode = CompilationMode.Partial(BaselineProfileMode.Require),
        setupBlock = {
            killProcess()
            launchOverview()
            device.openAppList()
        },
    ) {
        device.scrollList()
    }

    private fun measure(mode: CompilationMode) = benchmark.measureRepeated(
        packageName = PACKAGE,
        metrics = listOf(StartupTimingMetric()),
        iterations = 5,
        compilationMode = mode,
        startupMode = StartupMode.COLD,
        setupBlock = { pressHome() },
    ) {
        startActivityAndWait()
    }
}

// Navigation 3 ships without a profile. The app's other library profiles are merged separately.
private fun isProfileRule(rule: String): Boolean {
    val name = rule.substringAfter('L')
    return name.startsWith("androidx/navigation3/") ||
        (name.startsWith("li/mercury/adblocker/") && !name.startsWith("li/mercury/adblocker/benchmarksetup/"))
}

private fun seedReturningUser() {
    val device = UiDevice.getInstance(InstrumentationRegistry.getInstrumentation())
    val result = device.executeShellCommand("am broadcast --include-stopped-packages -n $PACKAGE/.benchmarksetup.ReturningUserSetupReceiver")
    check("result=-1" in result) { "Could not prepare the returning-user fixture: $result" }
    device.executeShellCommand("am force-stop $PACKAGE")
}

private fun MacrobenchmarkScope.launchOverview() {
    pressHome()
    startActivityAndWait()
    device.requireObject(By.text("ADBlocker"))
}

private fun UiDevice.openDestination(label: String) {
    waitForIdle()
    requireObject(By.clickable(true).hasDescendant(By.text(label))).click()
}

private fun UiDevice.openAppList() {
    openDestination("Apps")
    requireObject(By.textStartsWith("All ")).click()
    requireObject(By.scrollable(true))
}

private fun UiDevice.scrollList() {
    requireObject(By.scrollable(true)).swipe(Direction.UP, 0.7f)
    waitForIdle()
}

private fun UiDevice.requireObject(selector: BySelector): UiObject2 = checkNotNull(wait(Until.findObject(selector.pkg(PACKAGE)), TIMEOUT_MS)) {
    "No visible UI node matched $selector"
}
