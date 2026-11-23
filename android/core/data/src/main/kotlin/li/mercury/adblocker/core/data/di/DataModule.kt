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

package li.mercury.adblocker.core.data.di

import dagger.Binds
import dagger.Module
import dagger.Provides
import dagger.hilt.InstallIn
import dagger.hilt.components.SingletonComponent
import li.mercury.adblocker.core.data.AbdClient
import li.mercury.adblocker.core.data.DefaultAbdClient
import li.mercury.adblocker.core.data.apps.AndroidAppPlatformInfo
import li.mercury.adblocker.core.data.apps.AppPlatformInfo
import li.mercury.adblocker.core.data.repository.AbdActivityRepository
import li.mercury.adblocker.core.data.repository.AbdDaemonRepository
import li.mercury.adblocker.core.data.repository.AbdListsRepository
import li.mercury.adblocker.core.data.repository.AbdOverviewRepository
import li.mercury.adblocker.core.data.repository.AbdRulesRepository
import li.mercury.adblocker.core.data.repository.AbdStatsRepository
import li.mercury.adblocker.core.data.repository.AbdVpnAppRepository
import li.mercury.adblocker.core.data.repository.ActivityRepository
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.ContentDocumentWriter
import li.mercury.adblocker.core.data.repository.DaemonRepository
import li.mercury.adblocker.core.data.repository.DataStoreUserDataRepository
import li.mercury.adblocker.core.data.repository.DefaultAppsRepository
import li.mercury.adblocker.core.data.repository.DefaultFilterMethodRepository
import li.mercury.adblocker.core.data.repository.DefaultHttpsRepository
import li.mercury.adblocker.core.data.repository.DeviceRepository
import li.mercury.adblocker.core.data.repository.DiagnosticsRepository
import li.mercury.adblocker.core.data.repository.DocumentWriter
import li.mercury.adblocker.core.data.repository.FileRuleDraftStore
import li.mercury.adblocker.core.data.repository.FilterMethodRepository
import li.mercury.adblocker.core.data.repository.HttpsRepository
import li.mercury.adblocker.core.data.repository.ListsRepository
import li.mercury.adblocker.core.data.repository.ModuleRepository
import li.mercury.adblocker.core.data.repository.OverviewRepository
import li.mercury.adblocker.core.data.repository.RootDeviceRepository
import li.mercury.adblocker.core.data.repository.RootDiagnosticsRepository
import li.mercury.adblocker.core.data.repository.RootModuleRepository
import li.mercury.adblocker.core.data.repository.RuleDraftStore
import li.mercury.adblocker.core.data.repository.RulesRepository
import li.mercury.adblocker.core.data.repository.StatsRepository
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.data.repository.VpnAppRepository
import li.mercury.adblocker.core.data.transport.AbdTransports
import li.mercury.adblocker.core.data.transport.TransportSource
import java.time.Clock

@Module
@InstallIn(SingletonComponent::class)
internal abstract class DataModule {
    @Binds abstract fun abdClient(impl: DefaultAbdClient): AbdClient

    @Binds abstract fun transports(impl: AbdTransports): TransportSource

    @Binds abstract fun filterMethod(impl: DefaultFilterMethodRepository): FilterMethodRepository

    @Binds abstract fun https(impl: DefaultHttpsRepository): HttpsRepository

    @Binds abstract fun documentWriter(impl: ContentDocumentWriter): DocumentWriter

    @Binds abstract fun userDataRepository(impl: DataStoreUserDataRepository): UserDataRepository

    @Binds abstract fun daemonRepository(impl: AbdDaemonRepository): DaemonRepository

    @Binds abstract fun overviewRepository(impl: AbdOverviewRepository): OverviewRepository

    @Binds abstract fun appsRepository(impl: DefaultAppsRepository): AppsRepository

    @Binds abstract fun vpnAppRepository(impl: AbdVpnAppRepository): VpnAppRepository

    @Binds abstract fun statsRepository(impl: AbdStatsRepository): StatsRepository

    @Binds abstract fun listsRepository(impl: AbdListsRepository): ListsRepository

    @Binds abstract fun rulesRepository(impl: AbdRulesRepository): RulesRepository

    @Binds abstract fun ruleDraftStore(impl: FileRuleDraftStore): RuleDraftStore

    @Binds abstract fun activityRepository(impl: AbdActivityRepository): ActivityRepository

    @Binds abstract fun moduleRepository(impl: RootModuleRepository): ModuleRepository

    @Binds abstract fun appPlatformInfo(impl: AndroidAppPlatformInfo): AppPlatformInfo

    @Binds abstract fun deviceRepository(impl: RootDeviceRepository): DeviceRepository

    @Binds abstract fun diagnosticsRepository(impl: RootDiagnosticsRepository): DiagnosticsRepository

    companion object {
        /** Resolve the device's current time zone each time the stats are read. */
        @Provides fun clock(): Clock = Clock.systemDefaultZone()
    }
}
