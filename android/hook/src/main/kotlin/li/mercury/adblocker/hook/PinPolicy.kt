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

import java.security.cert.CertificateException
import java.security.cert.CertificateParsingException
import java.security.cert.X509Certificate
import java.util.Locale

/** The validation Android gives every connection of this app. */
internal fun interface PlatformTrust {
    /**
     * The chain the platform builds from [chain], ending in its trust
     * anchor, or a [CertificateException] where it does not trust it.
     *
     * Given the socket or engine of the handshake as [connection], it also
     * checks the chain as Android's trust manager does for that
     * connection, which includes the server's name where the app asked for
     * that check.
     */
    fun validate(chain: List<X509Certificate>, authType: String, host: String?, connection: Any?): List<X509Certificate>
}

/**
 * Decides whether a pin check that failed may pass: only for a chain the
 * platform validates as it would for any connection of the app, and whose
 * trust anchor is ADBlocker's root in this process ([LocalAuthority]). A
 * check that fails on the server's name stays failed.
 */
internal class PinPolicy(private val authority: LocalAuthority, private val platform: PlatformTrust) {

    /** Whether [validated], a chain the platform has already validated, ends in a local root. */
    fun endsInLocalRoot(validated: List<*>): Boolean {
        val anchor = validated.lastOrNull() as? X509Certificate ?: return false
        // The name first: other chains are told apart without reading the stores.
        if (!LocalAuthority.named(anchor)) return false
        return authority.roots()?.isAnchor(anchor) == true
    }

    /**
     * The chain the platform builds from [presented] if it validates and
     * ends in a local root, and its first certificate names [host], or
     * null. [authType], [host] and [connection] are those of the handshake
     * where known.
     */
    fun validatedToLocalRoot(
        presented: List<*>,
        authType: String?,
        host: String?,
        connection: Any? = null,
    ): List<X509Certificate>? {
        val chain = presented.map { it as? X509Certificate ?: return null }
        if (chain.none(LocalAuthority::named)) return null
        // Where the process has no roots of ours, nothing is validated.
        if (authority.roots() == null) return null
        val validated = try {
            platform.validate(chain, authType ?: DEFAULT_AUTH_TYPE, host, connection)
        } catch (_: CertificateException) {
            return null
        }
        if (!endsInLocalRoot(validated)) return null
        // The app's check may have failed on the server's name: that stands.
        if (host != null && !namesHost(validated.first(), host)) return null
        return validated
    }

    private companion object {
        // Android checks only that it is given one; OkHttp passes the same
        // when it asks the platform for a chain.
        const val DEFAULT_AUTH_TYPE = "RSA"
    }
}

/**
 * Whether [certificate] names [host] in its subject alternative names: a
 * DNS name, or one whose first label is a wildcard for exactly one label,
 * or the same IP address written the same way. Anything else, a common
 * name included, does not count.
 */
internal fun namesHost(certificate: X509Certificate, host: String): Boolean {
    val wanted = host.trimEnd('.').lowercase(Locale.ROOT)
    val names = try {
        certificate.subjectAlternativeNames
    } catch (_: CertificateParsingException) {
        null
    } ?: return false
    return names.any { entry ->
        val name = (entry.getOrNull(1) as? String)?.trimEnd('.')?.lowercase(Locale.ROOT)
        when {
            name == null -> false
            entry[0] == DNS_NAME -> name == wanted || coversOneLabel(name, wanted)
            entry[0] == IP_ADDRESS -> name == wanted.removePrefix("[").removeSuffix("]")
            else -> false
        }
    }
}

private fun coversOneLabel(pattern: String, host: String): Boolean {
    if (!pattern.startsWith("*.")) return false
    val parent = pattern.substring(1)
    return host.endsWith(parent) && host.indexOf('.') == host.length - parent.length && host.indexOf('.') > 0
}

private const val DNS_NAME = 2
private const val IP_ADDRESS = 7
