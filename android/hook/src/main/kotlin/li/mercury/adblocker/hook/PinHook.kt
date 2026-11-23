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

package li.mercury.adblocker.hook

import android.util.Log
import io.github.libxposed.api.XposedModule
import io.github.libxposed.api.XposedModuleInterface.PackageReadyParam
import java.util.concurrent.atomic.AtomicBoolean

/**
 * LSPosed entry point, loaded into the apps the user enables the module
 * for in LSPosed (`META-INF/xposed/java_init.list`; there is no fixed
 * scope).
 *
 * An app that checks the server's certificate itself, against a pin or a
 * trust store of its own, rejects the certificate the root module signs
 * when it decrypts the app's HTTPS, and the module then leaves that app's
 * connections to the host encrypted. In such an app, this lets those
 * checks accept a chain that Android has validated as it validates any
 * connection of the app, and whose trust anchor is the module's root in
 * this process ([PinPolicy]). Every other chain meets the app's checks as
 * they are, and in a process the module has not given its root, nothing
 * passes that would not have.
 */
class PinHook : XposedModule() {
    private val platformHooked = AtomicBoolean(false)
    private val hooks by lazy { PinHooks(this, AndroidPlatformTrust()) }

    override fun onPackageReady(param: PackageReadyParam) {
        // Android's classes are shared by the packages of a process; each
        // package can bring its own OkHttp.
        if (platformHooked.compareAndSet(false, true)) install("Android's") { hooks.installPlatform() }
        install("OkHttp's") { hooks.installOkHttp(param.classLoader) }
    }

    /** Installs one set of hooks, and logs what became of it alone. */
    private inline fun install(whose: String, hook: () -> Int) {
        try {
            val hooked = hook()
            if (hooked > 0) log(Log.INFO, PinHooks.TAG, "hooked $hooked of $whose certificate checks")
        } catch (e: Throwable) {
            // These checks stay as the app has them; hooks already
            // installed stay installed.
            log(Log.WARN, PinHooks.TAG, "could not hook $whose certificate checks (${e.javaClass.simpleName})")
        }
    }
}
