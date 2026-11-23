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

import io.github.libxposed.api.XposedInterface
import java.lang.reflect.Method
import java.security.cert.CertificateException
import javax.net.ssl.SSLPeerUnverifiedException

// Each hooker is a declared class rather than a lambda: R8 cannot see which
// method of a lambda implements Hooker.intercept, as the Xposed API is not
// there when the app is built, and the keep rule for this package covers
// declared classes (consumer-rules.pro).

/**
 * Network Security Config's pin check
 * (`NetworkSecurityTrustManager.checkPins(List)`), which is given the chain
 * the platform has just validated.
 */
internal class ValidatedChainHooker(private val policy: PinPolicy) : XposedInterface.Hooker {
    override fun intercept(chain: XposedInterface.Chain): Any? = relaxed(
        check = { chain.proceed() },
        isCheckFailure = { it is CertificateException },
        accept = { if (policy.endsInLocalRoot(chain.args[0] as List<*>)) Accepted(null) else null },
    )
}

/**
 * A trust manager's `checkServerTrusted`, in the forms Android calls
 * ([ServerCheck]). The forms that return the validated chain return the
 * platform's.
 *
 * A trust manager [skip] names is left to fail.
 */
internal class TrustManagerHooker(
    private val policy: PinPolicy,
    private val skip: (Any?) -> Boolean,
) : XposedInterface.Hooker {
    override fun intercept(chain: XposedInterface.Chain): Any? = relaxed(
        check = { chain.proceed() },
        isCheckFailure = { it is CertificateException },
        accept = { accept(chain) },
    )

    private fun accept(chain: XposedInterface.Chain): Accepted? {
        if (skip(chain.thisObject)) return null
        val method = chain.executable as Method
        val check = ServerCheck.of(method.parameterTypes, chain.args) ?: return null
        val validated = policy.validatedToLocalRoot(check.chain, check.authType, check.host, check.connection)
            ?: return null
        val returnsChain = method.returnType == List::class.java
        return Accepted(if (returnsChain) validated else null)
    }
}

/**
 * OkHttp's `CertificatePinner.check(String, List)` and, from OkHttp 4,
 * `check$okhttp(String, () -> List)`: the host, then the chain or a
 * function that cleans it.
 */
internal class OkHttpPinHooker(private val policy: PinPolicy) : XposedInterface.Hooker {
    override fun intercept(chain: XposedInterface.Chain): Any? = relaxed(
        check = { chain.proceed() },
        isCheckFailure = { it is SSLPeerUnverifiedException },
        accept = { accept(chain) },
    )

    private fun accept(chain: XposedInterface.Chain): Accepted? {
        val host = chain.args[0] as? String ?: return null
        val type = (chain.executable as Method).parameterTypes[1]
        val presented = certificatesOf(chain.args[1], type) ?: return null
        return policy.validatedToLocalRoot(presented, null, host)?.let { Accepted(null) }
    }
}

/**
 * `SSLContext.init`, where the app hands over its own trust managers: they
 * are passed to [onTrustManagers] once the context is initialized.
 */
internal class SslContextHooker(private val onTrustManagers: (Array<*>) -> Unit) : XposedInterface.Hooker {
    override fun intercept(chain: XposedInterface.Chain): Any? {
        val result = chain.proceed()
        val managers = chain.args[1] as? Array<*> ?: return result
        try {
            onTrustManagers(managers)
        } catch (_: Throwable) {
            // Best effort: the context is initialized as the app asked, and
            // the app's own checks stay as they are.
        }
        return result
    }
}
