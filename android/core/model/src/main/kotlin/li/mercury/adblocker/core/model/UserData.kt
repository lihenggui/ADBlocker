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

package li.mercury.adblocker.core.model

enum class ThemeMode { System, Light, Dark }

/** How often the filter lists are updated in the background. */
enum class UpdateInterval(val days: Int) {
    Daily(1),
    EveryThreeDays(3),
    Weekly(7),
    Never(0),
}

data class UserData(
    val themeMode: ThemeMode = ThemeMode.System,
    /** Colors from the wallpaper; off, the app's own scheme. */
    val useDynamicColor: Boolean = true,
    val updateInterval: UpdateInterval = UpdateInterval.Daily,
    val updateOnWifiOnly: Boolean = true,
    val showSystemApps: Boolean = false,
    val onboardingDone: Boolean = false,
    /** Suggestions the user dismissed, by key, not shown again. */
    val dismissedTips: Set<String> = emptySet(),
    /**
     * Chosen during onboarding, before the module ran: applied once it runs
     * (after the restart Magisk and APatch need), then cleared.
     */
    val pendingApps: Set<String> = emptySet(),
    val pendingFilteringScope: FilteringScope? = null,
    /** Set when setup could not show all work apps. Old deferred setups use the safe default. */
    val pendingPreserveWorkApps: Boolean = true,
    /** Lists turned off during onboarding, likewise. */
    val pendingDisabledLists: Set<String> = emptySet(),
    /** Lists selected during onboarding, likewise. */
    val pendingEnabledLists: Set<String> = emptySet(),
    /** The list pane was dragged to half the window, beside a detail: kept across launches. */
    val listPaneWide: Boolean = false,
    /** Chosen during setup; null before a method is selected. */
    val filterMethod: FilterMethod? = null,
    /** The user requested root mode. The VPN stays in use until setup is ready. */
    val rootSwitchPending: Boolean = false,
    /** Cancellation is waiting for the previous root app selection to be restored. */
    val rootSwitchCancelling: Boolean = false,
    /** Root mode was published and its rollback files still need to be removed. */
    val rootHandoffCleanupPending: Boolean = false,
) {
    val hasPendingLists: Boolean get() = pendingDisabledLists.isNotEmpty() || pendingEnabledLists.isNotEmpty()

    /** The method in use. Root is the initial method before setup. */
    val method: FilterMethod get() = filterMethod ?: FilterMethod.Root

    val capabilities: Capabilities get() = Capabilities(method)
}
