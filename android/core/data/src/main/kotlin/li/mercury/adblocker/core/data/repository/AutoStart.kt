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
import kotlinx.coroutines.flow.first
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.UserData
import timber.log.Timber
import javax.inject.Inject
import javax.inject.Singleton

/**
 * Starts the daemon without the user asking, where nothing else did.
 *
 * A module this app installed is started at boot by a launcher that
 * KernelSU, Magisk and APatch run (module/direct.sh). Root from another
 * source runs no such launcher, and the daemon would stay stopped until the
 * user started it. Where a manager ran it, the daemon runs already and
 * nothing is done. A module in a root manager is the manager's to start,
 * and one its boot guard turned off is the user's to turn on again.
 */
@Singleton
class AutoStart @Inject constructor(
    private val userData: UserDataRepository,
    private val module: ModuleRepository,
) {
    /** What became of one occasion to start. */
    enum class Outcome {
        /** Nothing of this app's to start: root was not asked for. */
        NotAsked,

        /** The daemon runs already, or the module is not to be started from here. */
        NothingToStart,
        Started,

        /** Root was refused, or the daemon did not start. Left at that, for the user to start it. */
        Failed,
    }

    /**
     * Once the device has booted. Tried once for a boot: asking for root
     * shows the root manager's notice, or its prompt. So it is asked only
     * where the settings remember a module this app installed, and not to
     * find that out.
     */
    suspend fun afterBoot(): Outcome {
        val settings = userData.userData.first()
        if (!settings.inRootMode || !settings.moduleInstalledByApp) return Outcome.NotAsked
        return start("boot")
    }

    private suspend fun start(occasion: String): Outcome = try {
        val outcome = if (module.startUnasked()) Outcome.Started else Outcome.NothingToStart
        Timber.i("start at %s: %s", occasion, outcome)
        outcome
    } catch (e: CancellationException) {
        throw e
    } catch (e: Exception) {
        Timber.w("start at %s: %s", occasion, e.javaClass.simpleName)
        Outcome.Failed
    }
}

/** Root is the method before one is chosen: only a finished setup says that it is in use. */
internal val UserData.inRootMode: Boolean get() = onboardingDone && method == FilterMethod.Root
