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

import android.annotation.SuppressLint
import android.util.Log
import io.github.libxposed.api.XposedInterface
import io.github.libxposed.api.XposedInterface.ExceptionMode
import io.github.libxposed.api.error.XposedFrameworkError
import org.lsposed.hiddenapibypass.HiddenApiBypass
import java.lang.reflect.Method
import java.security.SecureRandom
import java.util.concurrent.ConcurrentHashMap
import javax.net.ssl.KeyManager
import javax.net.ssl.SSLContext
import javax.net.ssl.TrustManager
import javax.net.ssl.X509TrustManager

/**
 * Finds the certificate checks of the process and hooks each once.
 *
 * Android's classes are hooked by their known signatures, verified against
 * the platform sources of every version the app runs on; a class or
 * method that is not there is left alone. Their members are hidden API,
 * and are looked up through [HiddenApiBypass], which leaves the app's own
 * access to hidden API as it was.
 */
internal class PinHooks(private val xposed: XposedInterface, private val platform: AndroidPlatformTrust) {
    private val policy = PinPolicy(LocalAuthority(::processStores, ::processMountInfo), platform)
    private val hooked: MutableSet<Method> = ConcurrentHashMap.newKeySet()

    // The app's trust manager classes already looked at: an app makes few,
    // and the same ones for each SSLContext.
    private val looked: MutableSet<Class<*>> = ConcurrentHashMap.newKeySet()

    private val appTrustManagerHooker = TrustManagerHooker(policy) { false }

    // The app's default trust manager is left to fail: it is what the
    // policy validates with, and its pins are Network Security Config's.
    private val platformTrustManagerHooker = TrustManagerHooker(policy) { it === platform.manager }

    /** Hooks Android's checks, once per process. Returns how many were hooked. */
    fun installPlatform(): Int = listOf(
        ::hookNetworkSecurityPins,
        ::hookPlatformTrustManagers,
        ::hookSslContextInit,
    ).sumOf { install ->
        // Each on its own: a check that is not where it is looked for leaves
        // the others hooked.
        try {
            install()
        } catch (e: Exception) {
            warn("a certificate check is not where it was looked for", e)
            0
        } catch (e: LinkageError) {
            warn("a certificate check is not where it was looked for", e)
            0
        }
    }

    /**
     * Hooks OkHttp's pinner where the app has it under its own name, which
     * only an app that does not obfuscate OkHttp keeps.
     */
    fun installOkHttp(loader: ClassLoader): Int {
        val pinner = try {
            Class.forName(OKHTTP_PINNER, false, loader)
        } catch (_: ClassNotFoundException) {
            return 0
        }
        val hooker = OkHttpPinHooker(policy)
        return pinner.declaredMethods.filter(::isOkHttpCheck).count { hook(it, hooker) }
    }

    @SuppressLint("PrivateApi")
    private fun hookNetworkSecurityPins(): Int {
        val type = Class.forName("android.security.net.config.NetworkSecurityTrustManager")
        val checkPins = HiddenApiBypass.getDeclaredMethod(type, "checkPins", List::class.java)
        if (!hook(checkPins, ValidatedChainHooker(policy))) return 0
        // A private method can be compiled into its callers in the boot
        // image, where a hook on it is never reached; its callers are the
        // server checks of the same class.
        HiddenApiBypass.getDeclaredMethods(type)
            .filterIsInstance<Method>()
            .filter { it.name == "checkServerTrusted" }
            .forEach(::deoptimize)
        return 1
    }

    private fun deoptimize(method: Method) {
        try {
            if (!xposed.deoptimize(method)) warn("a certificate check could not be deoptimized", null)
        } catch (e: RuntimeException) {
            // The method keeps its compiled code: the hook may then not be
            // reached through it, and nothing else changes.
            warn("a certificate check could not be deoptimized", e)
        } catch (e: XposedFrameworkError) {
            warn("a certificate check could not be deoptimized", e)
        }
    }

    /**
     * Network Security Config's trust managers, which are also what an app
     * gets from a TrustManagerFactory given its own key store.
     */
    @SuppressLint("PrivateApi")
    private fun hookPlatformTrustManagers(): Int {
        val type = Class.forName("android.security.net.config.RootTrustManager")
        return HiddenApiBypass.getDeclaredMethods(type)
            .filterIsInstance<Method>()
            .filter(::isServerCheck)
            .count { hook(it, platformTrustManagerHooker) }
    }

    private fun hookSslContextInit(): Int {
        val init = SSLContext::class.java.getMethod(
            "init",
            Array<KeyManager>::class.java,
            Array<TrustManager>::class.java,
            SecureRandom::class.java,
        )
        return if (hook(init, SslContextHooker(::hookAppTrustManagers))) 1 else 0
    }

    /** Hooks the checks of the trust managers the app wrote, from its class up to Android's. */
    private fun hookAppTrustManagers(managers: Array<*>) {
        for (manager in managers.filterIsInstance<X509TrustManager>()) {
            var type: Class<*>? = manager.javaClass
            while (type != null && isAppClass(type) && looked.add(type)) {
                type.declaredMethods.filter(::isServerCheck).forEach { hook(it, appTrustManagerHooker) }
                type = type.superclass
            }
        }
    }

    private fun hook(method: Method, hooker: XposedInterface.Hooker): Boolean {
        // A method is tried once, so that one that cannot be hooked is not
        // tried, and reported, at every SSLContext.
        if (!hooked.add(method)) return false
        return try {
            xposed.hook(method).setExceptionMode(ExceptionMode.PROTECTIVE).intercept(hooker)
            true
        } catch (e: RuntimeException) {
            warn("a certificate check cannot be hooked", e)
            false
        } catch (e: XposedFrameworkError) {
            warn("a certificate check could not be hooked", e)
            false
        }
    }

    // Names no host, app or certificate: only what failed, and its type.
    private fun warn(what: String, e: Throwable?) = xposed.log(Log.WARN, TAG, if (e == null) what else "$what (${e.javaClass.simpleName})")

    companion object {
        const val TAG = "ADBlocker"
    }
}
