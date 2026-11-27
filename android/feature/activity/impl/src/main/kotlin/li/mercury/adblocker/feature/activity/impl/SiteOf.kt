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

package li.mercury.adblocker.feature.activity.impl

import java.net.IDN

/** A hosted tenant is a site of its own. An unrecognized suffix can only be trusted at the requested host. */
internal fun siteOf(host: String): String? {
    val normalized = host.removeSuffix(".").lowercase()
    if (isNumericAddress(normalized)) return normalized
    // DNS names may have a root label. Requests report international names in ASCII form.
    val name = normalized.removeSuffix(".")
    val labels = dnsLabels(name) ?: return null
    if (labels.size < 2) return null

    var start = 0
    var previous = 0
    for (label in labels) {
        val suffix = name.substring(start)
        val next = start + label.length + 1
        if ("!$suffix" in PublicSuffixRules.entries) return suffix
        if (suffix in PublicSuffixRules.entries || (next < name.length && "*.${name.substring(next)}" in PublicSuffixRules.entries)) {
            return if (start == 0) null else name.substring(previous)
        }
        previous = start
        start = next
    }
    return name
}

/**
 * The host spelling used in an exception for a recorded host: a DNS name or
 * an IPv4 address. An endpoint with a port is not a host a rule can name, and
 * the engine cannot keep an IPv6 exception for one app to its own address.
 */
internal fun exceptionHost(host: String): String? {
    if (host.any { it.code >= 128 || it == ':' }) return null
    val domain = host.lowercase().removeSuffix(".")
    return domain.takeIf { isIpv4(it) || dnsLabels(it) != null }
}

private fun dnsLabels(name: String): List<String>? {
    if (name.length > 253 || name.any { it !in 'a'..'z' && it !in '0'..'9' && it != '.' && it != '-' }) return null
    val labels = name.split('.')
    if (labels.any { it.length !in 1..63 || it.first() == '-' || it.last() == '-' }) return null
    if (labels.last().first() in '0'..'9') return null
    return labels
}

private object PublicSuffixRules {
    val entries = checkNotNull(javaClass.getResourceAsStream("/rules.txt")) { "Public Suffix List resource is missing" }
        .bufferedReader()
        .useLines { lines ->
            lines.map { rule ->
                if (rule.all { it.code < 128 }) rule else IDN.toASCII(rule, IDN.ALLOW_UNASSIGNED)
            }.toHashSet()
        }
}

private fun isNumericAddress(host: String): Boolean {
    if (':' !in host) return isIpv4(host)
    val address = host.substringBefore('%')
    if (address.length !in 2..45) return false
    val allowIpv4 = address.length == host.length
    val compression = address.indexOf("::")
    if (compression == -1) return ipv6GroupCount(address, allowIpv4) == 8
    val before = ipv6GroupCount(address.substring(0, compression), allowIpv4 = false) ?: return false
    val after = ipv6GroupCount(address.substring(compression + 2), allowIpv4) ?: return false
    return before + after < 8
}

private fun ipv6GroupCount(part: String, allowIpv4: Boolean): Int? {
    if (part.isEmpty()) return 0
    val groups = part.split(':')
    var count = 0
    for ((index, group) in groups.withIndex()) {
        count += when {
            group.length in 1..4 && group.all { it.digitToIntOrNull(16) != null } -> 1
            allowIpv4 && index == groups.lastIndex && isIpv4(group) -> 2
            else -> return null
        }
    }
    return count
}

private fun isIpv4(host: String): Boolean {
    if (host.length !in 7..15 || host.first().digitToIntOrNull() == null) return false
    val octets = host.split('.')
    return octets.size == 4 && octets.all { octet ->
        if (octet.length !in 1..3 || (octet.length > 1 && octet.first() == '0')) return@all false
        var value = 0
        for (digit in octet) {
            value = value * 10 + (digit.digitToIntOrNull() ?: return@all false)
        }
        value <= 255
    }
}
