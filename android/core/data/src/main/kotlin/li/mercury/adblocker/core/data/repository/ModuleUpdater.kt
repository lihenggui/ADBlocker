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

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import li.mercury.adblocker.core.data.toLoadFailure
import li.mercury.adblocker.core.model.LoadFailure
import timber.log.Timber
import java.util.concurrent.atomic.AtomicBoolean
import javax.inject.Inject
import javax.inject.Singleton

/** Bringing the installed module up to date with the one this app carries. */
sealed interface ModuleUpdate {
    /** Nothing in hand: up to date as far as is known, or not looked at. */
    data object Idle : ModuleUpdate

    data object Updating : ModuleUpdate

    /** The last update did not install; [ModuleUpdater.update] tries again. */
    data class Failed(val failure: LoadFailure) : ModuleUpdate
}

/**
 * Replaces a module older than this app with the one the app carries.
 *
 * The app installs the module, and an update of the app brings a new one
 * that nothing else installs: the old daemon goes on running, and rejects
 * what the new app asks of it. Opened, the app therefore installs its own
 * once, without being asked, where that turns nothing on that was off; and
 * again when asked, as for a command the old daemon did not know
 * ([LoadFailure.ModuleOutdated]).
 */
@Singleton
class ModuleUpdater @Inject constructor(
    private val userData: UserDataRepository,
    private val module: ModuleRepository,
) {
    private val update = MutableStateFlow<ModuleUpdate>(ModuleUpdate.Idle)
    val state: StateFlow<ModuleUpdate> = update.asStateFlow()

    private val looked = AtomicBoolean(false)
    private val installing = Mutex()

    /**
     * When the app is opened in root mode, once for a process. Nothing is
     * said of a module that could not be looked at: the screens say what
     * they cannot read.
     */
    suspend fun updateIfOutdated() {
        if (!userData.userData.first().inRootMode || looked.getAndSet(true)) return
        val needed = try {
            module.needsUpdate()
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            Timber.w("module update: not checked: %s", e.javaClass.simpleName)
            return
        }
        if (needed) install()
    }

    /** On request. Installing turns the module on, as when the user installs it. */
    suspend fun update() = install()

    private suspend fun install() = installing.withLock {
        update.value = ModuleUpdate.Updating
        update.value = try {
            module.installBundled()
            ModuleUpdate.Idle
        } catch (e: CancellationException) {
            update.value = ModuleUpdate.Idle
            throw e
        } catch (e: Exception) {
            ModuleUpdate.Failed(e.toLoadFailure())
        }
        Timber.i("module update: %s", update.value::class.simpleName)
    }
}
