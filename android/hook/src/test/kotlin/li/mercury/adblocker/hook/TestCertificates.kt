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

import org.bouncycastle.asn1.x500.X500Name
import org.bouncycastle.asn1.x500.X500NameBuilder
import org.bouncycastle.asn1.x500.style.BCStyle
import org.bouncycastle.asn1.x509.BasicConstraints
import org.bouncycastle.asn1.x509.Extension
import org.bouncycastle.asn1.x509.GeneralName
import org.bouncycastle.asn1.x509.GeneralNames
import org.bouncycastle.asn1.x509.KeyUsage
import org.bouncycastle.cert.jcajce.JcaX509CertificateConverter
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder
import java.io.File
import java.math.BigInteger
import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.cert.CertPathBuilder
import java.security.cert.CertStore
import java.security.cert.CertificateException
import java.security.cert.CollectionCertStoreParameters
import java.security.cert.PKIXBuilderParameters
import java.security.cert.PKIXCertPathBuilderResult
import java.security.cert.TrustAnchor
import java.security.cert.X509CertSelector
import java.security.cert.X509Certificate
import java.time.Duration
import java.time.Instant
import java.util.Base64
import java.util.Date
import java.util.concurrent.atomic.AtomicLong
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLEngine

/**
 * A mount table as a filtered app's shows the module's copies over
 * [stores], from a Pixel emulator with Android 15.
 */
internal fun moduleMountInfo(stores: Stores, systemCopy: String = ModuleMounts.SYSTEM_COPY): String = """
    2625 2624 254:24 / / ro,relatime master:1 - ext4 /dev/block/dm-24 ro,seclabel
    2946 2625 254:46 / /data rw,nosuid,nodev,noatime master:34 - ext4 /dev/block/dm-46 rw,seclabel,resgid=1065,errors=panic
    2947 2946 254:46 /data /data/user/0 rw,nosuid,nodev,noatime master:34 - ext4 /dev/block/dm-46 rw,seclabel,resgid=1065,errors=panic
    3472 2915 254:46 $systemCopy ${stores.system} ro,nosuid,nodev,noexec,noatime master:34 - ext4 /dev/block/dm-46 rw,seclabel,resgid=1065,errors=panic
    3617 2946 254:46 ${ModuleMounts.USER_COPY} ${stores.user} ro,nosuid,nodev,noexec,noatime master:34 - ext4 /dev/block/dm-46 rw,seclabel,resgid=1065,errors=panic
""".trimIndent()

/** An engine for [host] that checks the server's name as an app can ask (`setEndpointIdentificationAlgorithm`). */
internal fun identifyingEngine(host: String): SSLEngine = SSLContext.getDefault().createSSLEngine(host, 443).apply {
    sslParameters = sslParameters.apply { endpointIdentificationAlgorithm = "HTTPS" }
}

/** A certificate and the key that signs with it. */
internal class Issuer(val certificate: X509Certificate, val keys: KeyPair)

/**
 * An authority made as `abd-engine/src/ca.rs` makes one: a system root, a
 * user root, the user root's key certified by the system root, and leaves
 * signed by the user key and sent with that cross-certificate.
 */
internal class TestAuthority {
    val system = selfSigned("ADBlocker system CA")
    val user = selfSigned("ADBlocker user CA")
    val cross = issue(rootName("ADBlocker user CA"), user.keys, system, ca = true)

    fun presented(host: String): List<X509Certificate> = listOf(leaf(host, user), cross)

    /** Writes the roots where a store mounted by the module has them. */
    fun mountInto(stores: Stores) {
        writePem(File(stores.system, LocalAuthority.SYSTEM_ROOT_FILE), system.certificate)
        writePem(File(stores.user, LocalAuthority.USER_ROOT_FILE), user.certificate)
    }
}

/** A public root and its chains, as an ordinary server presents them. */
internal class PublicAuthority(name: String = "Example Root") {
    val root = selfSigned(name, organization = "Example")

    fun presented(host: String): List<X509Certificate> = listOf(leaf(host, root))
}

internal fun keys(): KeyPair = KeyPairGenerator.getInstance("EC").apply { initialize(256) }.generateKeyPair()

/** The subject `ca.rs` gives a root: UTF8String values, the organization first. */
internal fun rootName(commonName: String, organization: String = "ADBlocker"): X500Name = X500NameBuilder(BCStyle.INSTANCE)
    .addRDN(BCStyle.O, organization)
    .addRDN(BCStyle.CN, commonName)
    .build()

internal fun selfSigned(commonName: String, organization: String = "ADBlocker", keys: KeyPair = keys()): Issuer {
    val name = rootName(commonName, organization)
    val certificate = sign(name, name, keys, keys, ca = true, host = null)
    return Issuer(certificate, keys)
}

internal fun issue(subject: X500Name, keys: KeyPair, issuer: Issuer, ca: Boolean): X509Certificate = sign(
    subject,
    X500Name.getInstance(issuer.certificate.subjectX500Principal.encoded),
    keys,
    issuer.keys,
    ca,
    host = null,
)

internal fun leaf(host: String, issuer: Issuer): X509Certificate = sign(
    X500NameBuilder(BCStyle.INSTANCE).addRDN(BCStyle.CN, host).build(),
    X500Name.getInstance(issuer.certificate.subjectX500Principal.encoded),
    keys(),
    issuer.keys,
    ca = false,
    host = host,
)

private val serials = AtomicLong(1)

private fun sign(
    subject: X500Name,
    issuer: X500Name,
    keys: KeyPair,
    signer: KeyPair,
    ca: Boolean,
    host: String?,
): X509Certificate {
    val now = Instant.now()
    val builder = JcaX509v3CertificateBuilder(
        issuer,
        BigInteger.valueOf(serials.getAndIncrement()),
        Date.from(now - Duration.ofDays(2)),
        Date.from(now + Duration.ofDays(180)),
        subject,
        keys.public,
    )
    if (ca) {
        builder.addExtension(Extension.basicConstraints, true, BasicConstraints(true))
        builder.addExtension(Extension.keyUsage, true, KeyUsage(KeyUsage.keyCertSign or KeyUsage.cRLSign))
    } else {
        builder.addExtension(Extension.keyUsage, true, KeyUsage(KeyUsage.digitalSignature))
        val type = if (host.orEmpty().all { it.isDigit() || it == '.' }) GeneralName.iPAddress else GeneralName.dNSName
        builder.addExtension(Extension.subjectAlternativeName, false, GeneralNames(GeneralName(type, host)))
    }
    val holder = builder.build(JcaContentSignerBuilder("SHA256withECDSA").build(signer.private))
    return JcaX509CertificateConverter().getCertificate(holder)
}

internal fun writePem(file: File, certificate: X509Certificate) {
    file.parentFile.mkdirs()
    val body = Base64.getMimeEncoder(64, "\n".toByteArray()).encodeToString(certificate.encoded)
    file.writeText("-----BEGIN CERTIFICATE-----\n$body\n-----END CERTIFICATE-----\n")
}

/**
 * Validation as the platform does it: a path from the presented chain to
 * one of [anchors], returned with the anchor last.
 */
internal class PkixTrust(private vararg val anchors: X509Certificate) : PlatformTrust {
    var calls = 0
        private set
    var connection: Any? = null
        private set

    override fun validate(
        chain: List<X509Certificate>,
        authType: String,
        host: String?,
        connection: Any?,
    ): List<X509Certificate> {
        calls++
        this.connection = connection
        // As Conscrypt checks the forms with an engine: the leaf must name
        // the engine's host when the app turned on endpoint identification.
        val engine = connection as? SSLEngine
        if (engine != null &&
            engine.sslParameters.endpointIdentificationAlgorithm.equals("HTTPS", ignoreCase = true) &&
            chain.first().subjectAlternativeNames.orEmpty().none { it[1] == engine.peerHost }
        ) {
            throw CertificateException("No subjectAltNames on the certificate match")
        }
        val target = X509CertSelector().apply { certificate = chain.first() }
        val parameters = PKIXBuilderParameters(anchors.map { TrustAnchor(it, null) }.toSet(), target).apply {
            isRevocationEnabled = false
            addCertStore(CertStore.getInstance("Collection", CollectionCertStoreParameters(chain)))
        }
        val result = try {
            CertPathBuilder.getInstance("PKIX").build(parameters) as PKIXCertPathBuilderResult
        } catch (e: java.security.GeneralSecurityException) {
            throw CertificateException(e)
        }

        @Suppress("UNCHECKED_CAST")
        val path = result.certPath.certificates as List<X509Certificate>
        return path + result.trustAnchor.trustedCert
    }
}
