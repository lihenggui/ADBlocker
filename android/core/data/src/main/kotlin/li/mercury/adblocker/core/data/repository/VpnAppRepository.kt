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

package li.mercury.adblocker.core.data.repository

import li.mercury.adblocker.core.data.AbdClient
import li.mercury.adblocker.core.model.AppSelections
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.FilteredApp
import javax.inject.Inject

/** The selection and status reads needed to route apps and show the VPN notification. */
interface VpnAppRepository {
    suspend fun appSelections(): AppSelections

    suspend fun apps(): List<FilteredApp>

    suspend fun status(): DaemonStatus
}

internal class AbdVpnAppRepository @Inject constructor(private val abd: AbdClient) : VpnAppRepository {
    override suspend fun appSelections() = abd.appSelections()

    override suspend fun apps() = abd.apps()

    override suspend fun status() = abd.status()
}
