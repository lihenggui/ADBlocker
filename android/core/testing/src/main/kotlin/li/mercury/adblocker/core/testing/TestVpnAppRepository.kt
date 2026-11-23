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

package li.mercury.adblocker.core.testing

import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.repository.VpnAppRepository
import li.mercury.adblocker.core.model.AppSelections
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.FilteredApp

class TestVpnAppRepository(var daemonStatus: DaemonStatus) : VpnAppRepository {
    var selections = AppSelections()
    var chosen = emptyList<FilteredApp>()
    var selectionsFailure: AbdException? = null
    var appsFailure: AbdException? = null
    var statusFailure: AbdException? = null

    override suspend fun appSelections(): AppSelections {
        selectionsFailure?.let { throw it }
        return selections
    }

    override suspend fun apps(): List<FilteredApp> {
        appsFailure?.let { throw it }
        return chosen
    }

    override suspend fun status(): DaemonStatus {
        statusFailure?.let { throw it }
        return daemonStatus
    }
}
