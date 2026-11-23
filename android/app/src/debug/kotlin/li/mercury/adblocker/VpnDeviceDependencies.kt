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

import dagger.hilt.EntryPoint
import dagger.hilt.InstallIn
import dagger.hilt.components.SingletonComponent
import li.mercury.adblocker.core.data.AbdClient
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.HttpsRepository
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.data.vpn.VpnController
import li.mercury.adblocker.core.engine.EngineHost

/** Device checks drive the app controller and the native engine. */
@EntryPoint
@InstallIn(SingletonComponent::class)
interface VpnDeviceDependencies {
    fun userData(): UserDataRepository
    fun abd(): AbdClient
    fun engine(): EngineHost
    fun apps(): AppsRepository
    fun https(): HttpsRepository
    fun vpn(): VpnController
}
