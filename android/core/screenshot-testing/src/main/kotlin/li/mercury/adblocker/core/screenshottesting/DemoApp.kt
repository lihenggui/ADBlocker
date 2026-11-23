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

package li.mercury.adblocker.core.screenshottesting

import androidx.compose.ui.graphics.vector.ImageVector
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.AppIconSource
import li.mercury.adblocker.core.model.InstalledApp
import java.util.Locale

/**
 * The apps of the demo device the screenshots show. All are made up: the
 * packages are under example.com, which is reserved for examples, and no
 * name or icon is a real app's. A browser, news and games, a bank and
 * apps worth leaving alone, as a device has.
 *
 * Each has a launcher icon of its own, drawn by [DemoAppIcons]: a disc in
 * a gradient from [topColor] to [bottomColor], with [glyph] in [glyphColor].
 */
enum class DemoApp(
    val packageName: String,
    private val label: String,
    private val chineseLabel: String,
    internal val glyph: ImageVector,
    internal val topColor: Long,
    internal val bottomColor: Long,
    internal val glyphColor: Long = WHITE,
    val isSystem: Boolean = false,
) {
    Browser("com.example.wayfarer", "Wayfarer Browser", "远航浏览器", AdbIcons.Public, 0xFF4F7CFF, 0xFF2747B8),
    News("com.example.dailybrief", "Daily Brief", "每日简报", AdbIcons.ReceiptLong, 0xFFE8574A, 0xFFB02F25),
    Puzzle("com.example.tilepuzzle", "Tile Puzzle", "拼块谜题", AdbIcons.Apps, 0xFFFFB648, 0xFFE07E12, glyphColor = 0xFF4A2A00),
    Video("com.example.reel", "Reel", "短片", AdbIcons.PlayArrow, 0xFF8E5BE8, 0xFF5B2DB3),
    Weather("com.example.skyline", "Skyline Weather", "天际天气", AdbIcons.Schedule, 0xFF3EC1E8, 0xFF1A86B8),
    Recipes("com.example.pantry", "Pantry", "食材库", AdbIcons.Edit, 0xFF5CC27A, 0xFF2E8A4C),
    Bank("com.example.northbank", "North Bank", "北方银行", AdbIcons.AccountBalance, 0xFF1E3A5F, 0xFF0E1F36),
    Wallet("com.example.pocketpay", "Pocket Pay", "口袋支付", AdbIcons.Key, 0xFF19B394, 0xFF0B7A63),
    Notes("com.example.notes", "Notes", "便签", AdbIcons.Edit, 0xFFFFE07A, 0xFFF2C230, glyphColor = 0xFF4A3B00),
    Services("com.example.services", "Device Services", "设备服务", AdbIcons.Settings, 0xFF8A9A92, 0xFF5D6B64, isSystem = true),
    ;

    /** The label in the language of the test (`zh-rCN` or not). */
    val localizedLabel: String get() = if (Locale.getDefault().language == "zh") chineseLabel else label

    val installedApp: InstalledApp
        get() = InstalledApp(
            packageName = packageName,
            label = localizedLabel,
            icon = AppIconSource(packageName, "demo"),
            isSystem = isSystem,
        )

    companion object {
        fun of(packageName: String): DemoApp? = entries.firstOrNull { it.packageName == packageName }
    }
}

private const val WHITE = 0xFFFFFFFF
