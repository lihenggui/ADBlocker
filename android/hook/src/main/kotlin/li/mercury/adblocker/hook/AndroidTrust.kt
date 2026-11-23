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

import android.net.http.X509TrustManagerExtensions
import android.os.Process
import java.io.ByteArrayOutputStream
import java.io.File
import java.net.Socket
import java.security.KeyStore
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import javax.net.ssl.SSLEngine
import javax.net.ssl.TrustManagerFactory
import javax.net.ssl.X509ExtendedTrustManager
import javax.net.ssl.X509TrustManager

/**
 * The app's default validation: Network Security Config's trust manager,
 * with the trust anchors and pins the app declares.
 *
 * Asked of its own provider by name, as a security provider an app
 * installs in front of it (Google Play services' does) would otherwise
 * answer for the default.
 */
internal class AndroidPlatformTrust : PlatformTrust {
    // Made on first use: by then Android has set up the app's configuration.
    // An initializer that fails is tried again on the next use.
    val manager: X509TrustManager by lazy {
        val factory = TrustManagerFactory.getInstance("PKIX", NETWORK_SECURITY_PROVIDER)
        factory.init(null as KeyStore?)
        factory.trustManagers.filterIsInstance<X509TrustManager>().first()
    }

    private val extensions by lazy { X509TrustManagerExtensions(manager) }

    override fun validate(
        chain: List<X509Certificate>,
        authType: String,
        host: String?,
        connection: Any?,
    ): List<X509Certificate> {
        val certificates = chain.toTypedArray()
        val validated = extensions.checkServerTrusted(certificates, authType, host)
        // The forms with the handshake's socket or engine also check the
        // server's name where the app turned on endpoint identification.
        // This trust manager's own hooks leave its failures as they are,
        // and its pin check passes a chain that ends in a local root.
        if (connection is Socket || connection is SSLEngine) {
            val extended = manager as? X509ExtendedTrustManager
                ?: throw CertificateException("no trust manager for the connection")
            when (connection) {
                is Socket -> extended.checkServerTrusted(certificates, authType, connection)
                is SSLEngine -> extended.checkServerTrusted(certificates, authType, connection)
            }
        }
        return validated
    }

    private companion object {
        // android.security.net.config.NetworkSecurityConfigProvider
        const val NETWORK_SECURITY_PROVIDER = "AndroidNSSP"
    }
}

/** The certificate stores this process reads, where `abd/src/trust.rs` mounts its copies. */
internal fun processStores(): Stores = Stores(
    system = File(APEX_STORE).takeIf(File::isDirectory) ?: File(SYSTEM_STORE),
    user = File("/data/misc/user/${Process.myUid() / PER_USER_RANGE}/cacerts-added"),
)

/**
 * This process's mount table, or nothing where it is longer than any app's
 * is: a table cut short could miss the mount that lies over a store.
 */
internal fun processMountInfo(): String = File("/proc/self/mountinfo").inputStream().use { input ->
    val table = ByteArrayOutputStream()
    val buffer = ByteArray(8192)
    while (table.size() <= MAX_MOUNT_INFO_BYTES) {
        val read = input.read(buffer)
        if (read < 0) return@use table.toString(Charsets.UTF_8.name())
        table.write(buffer, 0, read)
    }
    ""
}

private const val APEX_STORE = "/apex/com.android.conscrypt/cacerts"
private const val SYSTEM_STORE = "/system/etc/security/cacerts"
private const val PER_USER_RANGE = 100_000
private const val MAX_MOUNT_INFO_BYTES = 4 shl 20
