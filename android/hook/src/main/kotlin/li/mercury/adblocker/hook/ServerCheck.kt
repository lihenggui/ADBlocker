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

import java.net.Socket
import java.security.cert.X509Certificate
import javax.net.ssl.SSLEngine
import javax.net.ssl.SSLSocket

/**
 * The arguments of a trust manager's `checkServerTrusted`, read by their
 * position in the form that was called. [connection] is the socket or
 * engine of the handshake, in the forms that take one.
 */
internal class ServerCheck(
    val chain: List<*>,
    val authType: String?,
    val host: String?,
    val connection: Any? = null,
) {
    companion object {
        private val CHAIN = Array<X509Certificate>::class.java
        private val STRING = String::class.java
        private val BYTES = ByteArray::class.java

        /** The forms Android calls, after the chain. */
        val FORMS: Set<List<Class<*>>> = setOf(
            listOf(STRING),
            listOf(STRING, STRING),
            listOf(STRING, Socket::class.java),
            listOf(STRING, SSLEngine::class.java),
            // Android 16: OCSP and SCT data, then the authentication type and host.
            listOf(BYTES, BYTES, STRING, STRING),
        )

        /** Whether [parameters] are those of a form Android calls. */
        fun isForm(parameters: Array<Class<*>>): Boolean = parameters.firstOrNull() == CHAIN && parameters.drop(1) in FORMS

        /** The arguments [args] of the form [parameters], or null for another form. */
        fun of(parameters: Array<Class<*>>, args: List<Any?>): ServerCheck? {
            if (!isForm(parameters) || args.size != parameters.size) return null
            val chain = (args[0] as? Array<*>)?.asList() ?: return null
            return when (parameters.size) {
                2 -> ServerCheck(chain, args[1] as? String, null)

                3 -> when (val third = args[2]) {
                    is String? -> ServerCheck(chain, args[1] as? String, third)
                    else -> ServerCheck(chain, args[1] as? String, peerHost(third), third)
                }

                else -> ServerCheck(chain, args[3] as? String, args[4] as? String)
            }
        }
    }
}

/** The host a handshake is with, from a trust manager's [Socket] or [SSLEngine] argument. */
internal fun peerHost(argument: Any?): String? = when (argument) {
    is SSLSocket -> argument.handshakeSession?.peerHost
    is SSLEngine -> argument.handshakeSession?.peerHost ?: argument.peerHost
    else -> null
}
