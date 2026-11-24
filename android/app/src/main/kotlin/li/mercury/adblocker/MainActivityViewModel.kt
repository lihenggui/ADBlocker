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

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import li.mercury.adblocker.core.common.result.launchCatching
import li.mercury.adblocker.core.data.repository.AutoStart
import li.mercury.adblocker.core.data.repository.ListUpdateScheduler
import li.mercury.adblocker.core.data.repository.PendingSetup
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.model.UserData
import javax.inject.Inject

@HiltViewModel
class MainActivityViewModel @Inject constructor(
    private val userDataRepository: UserDataRepository,
    private val pendingSetup: PendingSetup,
    private val scheduler: ListUpdateScheduler,
    private val autoStart: AutoStart,
) : ViewModel() {
    /** The settings; null until read, so that the first frame already has the right colors. */
    val userData: StateFlow<UserData?> = userDataRepository.userData
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    /** The list pane's width, dragged by the user, kept for the next launch. */
    fun setListPaneWide(wide: Boolean) {
        if (userData.value?.listPaneWide == wide) return
        viewModelScope.launch { userDataRepository.setListPaneWide(wide) }
    }

    private var applyingSetup: Job? = null

    /** Applies choices after the module has been installed or the device restarted. */
    fun applyPendingSetup() {
        if (applyingSetup?.isActive == true) return
        applyingSetup = viewModelScope.launchCatching("pending setup") {
            // A daemon nothing started since the device did, before what waits for it.
            autoStart.whenOpened()
            if (pendingSetup.applyIfReady()) scheduler.updateSoon()
        }
    }
}
