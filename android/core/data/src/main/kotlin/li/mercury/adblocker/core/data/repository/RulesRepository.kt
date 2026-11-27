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

import android.content.Context
import android.net.Uri
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.withContext
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.IO
import li.mercury.adblocker.core.common.dispatchers.Dispatcher
import li.mercury.adblocker.core.data.AbdClient
import li.mercury.adblocker.core.model.RuleCheckResult
import li.mercury.adblocker.core.model.RulesCheck
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import javax.inject.Inject

/** The most the daemon takes of a list or of the user's rules (filter::MAX_LIST). */
const val MAX_RULES_BYTES = 16 shl 20

/** A file of rules larger than the daemon takes. */
class RulesTooLargeException : IOException("larger than $MAX_RULES_BYTES bytes")

/** The user's own rules and trusted sites. */
interface RulesRepository {
    suspend fun userRules(): String

    /** Replaces the user's rules; the daemon applies them at once. */
    suspend fun save(text: String)

    /** Which lines of [text] would not be applied, and why. */
    suspend fun check(text: String): RulesCheck

    /** What the lists say about a request to [url], made by [app] when it is known. */
    suspend fun checkUrl(url: String, page: String?, kind: String?, app: String? = null): RuleCheckResult

    /** Which whole-domain rule matches [host] in [app]. */
    suspend fun checkHost(host: String, app: String): RuleCheckResult

    /** Adds a rule that blocks [domain] and every name below it, in the package [app] only when it is given. */
    suspend fun blockDomain(domain: String, app: String? = null)

    /** Adds an exception for [domain] and every name below it, in the package [app] only when it is given. */
    suspend fun allowDomain(domain: String, app: String? = null)

    /** Appends a single rule if absent, returning whether it was added. */
    suspend fun addRule(rule: String): Boolean

    /** Removes exact matches for the rule, returning whether it was present. */
    suspend fun removeRule(rule: String): Boolean

    /**
     * Adds the rules in [uri], a file the user picked, to the user's own, and
     * returns how many it had; 0 for a file with none, which changes nothing.
     *
     * @throws RulesTooLargeException for a file larger than the daemon takes.
     */
    suspend fun import(uri: Uri): Int

    /** Writes the user's rules into [uri], a file the user picked. */
    suspend fun export(uri: Uri)

    /** Trusted sites: never decrypted, never blocked. */
    suspend fun trustedSites(): List<String>

    suspend fun trust(host: String)

    suspend fun untrust(host: String)
}

/** Checks a manually entered address, using HTTPS by default and leaving a blank page unknown. */
suspend fun RulesRepository.checkEnteredUrl(url: String, page: String, kind: String): RuleCheckResult {
    val address = url.trim().let { if ("://" in it) it else "https://$it" }
    return checkUrl(address, page.trim().ifEmpty { null }, kind)
}

internal class AbdRulesRepository @Inject constructor(
    @ApplicationContext private val context: Context,
    private val abd: AbdClient,
    private val changes: DataChanges,
    private val documents: DocumentWriter,
    @Dispatcher(IO) private val ioDispatcher: CoroutineDispatcher,
) : RulesRepository {
    override suspend fun userRules(): String = abd.userRules()

    override suspend fun save(text: String) = changes.changing { abd.setUserRules(text) }

    override suspend fun check(text: String): RulesCheck = abd.checkRules(text)

    override suspend fun checkUrl(url: String, page: String?, kind: String?, app: String?): RuleCheckResult = abd.check(url, page, kind, app)

    override suspend fun checkHost(host: String, app: String): RuleCheckResult = abd.checkHost(host, app)

    override suspend fun blockDomain(domain: String, app: String?) = appendDomainRule(blockRule(domain, app))

    override suspend fun allowDomain(domain: String, app: String?) = appendDomainRule(allowRule(domain, app))

    private suspend fun appendDomainRule(rule: String) = changes.changing { abd.appendUserRules("$rule\n", unique = true) }

    override suspend fun addRule(rule: String): Boolean = changes.changing { abd.appendUniqueUserRule("$rule\n") }

    override suspend fun removeRule(rule: String): Boolean = changes.changing { abd.removeUserRules("$rule\n") }

    override suspend fun import(uri: Uri): Int {
        val text = withContext(ioDispatcher) {
            val input = context.contentResolver.openInputStream(uri) ?: throw IOException("cannot read the file")
            input.use { readLimited(it, MAX_RULES_BYTES) }.decodeToString()
        }
        if (text.isBlank()) return 0
        changes.changing { abd.appendUserRules(text) }
        return text.lines().count { it.isNotBlank() && !it.startsWith('!') }
    }

    override suspend fun export(uri: Uri) {
        documents.write(uri, abd.userRules().toByteArray())
    }

    override suspend fun trustedSites(): List<String> = abd.allowList()

    override suspend fun trust(host: String) = changes.changing { abd.allow(host) }

    override suspend fun untrust(host: String) = changes.changing { abd.disallow(host) }
}

/**
 * All of [input], or [RulesTooLargeException] once it is longer than [max]:
 * a file picked by mistake must not fill the memory.
 */
internal fun readLimited(input: InputStream, max: Int): ByteArray {
    val out = ByteArrayOutputStream()
    val buffer = ByteArray(minOf(max + 1, READ_BUFFER))
    while (out.size() <= max) {
        val count = input.read(buffer, 0, minOf(buffer.size, max + 1 - out.size()))
        if (count < 0) return out.toByteArray()
        out.write(buffer, 0, count)
    }
    throw RulesTooLargeException()
}

private const val READ_BUFFER = 64 * 1024

/**
 * The rule that blocks [domain] and every name below it, whatever the
 * request: in every app, or in the package [app] only, in lower case as the
 * engine names it.
 */
fun blockRule(domain: String, app: String? = null): String = domainRule(domain, app)

/** The exception for [domain] and every name below it, optionally restricted to [app]. */
fun allowRule(domain: String, app: String? = null): String = "@@${domainRule(domain, app)}"

private fun domainRule(domain: String, app: String?): String {
    val host = "||${domain.trim().trimStart('.').lowercase()}^"
    return if (app == null) host else "$host\$app=${app.lowercase()}"
}

enum class HostInput { Site, RulePreview }

/**
 * [input] as a host name, or null if it is not one: a pasted address is cut
 * down to its host.
 */
fun hostOf(input: String, mode: HostInput = HostInput.Site): String? {
    val authority = input.trim().substringAfter("://").substringBefore('/')
    if (mode == HostInput.RulePreview) {
        // The preview accepts loose input while the user is still editing.
        val host = authority.substringBefore(':').trim('.').lowercase()
        return host.takeIf { '.' in it && it.all { c -> c.isLetterOrDigit() || c == '.' || c == '-' } }
    }
    val host = authority.substringBefore('?').substringBefore('#').substringAfterLast('@')
        .substringBefore(':').trimEnd('.').lowercase()
    val valid = host.length in 1..253 && '.' in host &&
        host.split('.').all { label -> label.isNotEmpty() && label.length <= 63 && label.all { it.isLetterOrDigit() || it == '-' } }
    return host.takeIf { valid }
}
