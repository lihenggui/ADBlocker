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

package li.mercury.adblocker.core.designsystem.theme

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialExpressiveTheme
import androidx.compose.material3.MotionScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalContext

/**
 * Material 3 Expressive theme.
 *
 * From Android 12 (API 31), the dynamic palette follows the wallpaper and
 * contrast setting by default. Android 10 and 11 use the app's own scheme,
 * as does turning [useDynamicColor] off. Components use the expressive
 * motion scheme, as the specification recommends for most products.
 * The app's navigation sets screen transitions separately. The type scale carries its emphasized styles
 * ([ExpressiveTypography]).
 *
 * The app's own scheme has one contrast level: the design names medium and
 * high contrast variants but gives no values for them, and dynamic color
 * follows the system's contrast setting by itself.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun ADBlockerTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    useDynamicColor: Boolean = true,
    content: @Composable () -> Unit,
) {
    val context = LocalContext.current
    // Wallpaper colors came with Android 12; before, the app's own.
    val dynamic = useDynamicColor && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
    val colorScheme = when {
        dynamic && darkTheme -> dynamicDarkColorScheme(context)
        dynamic -> dynamicLightColorScheme(context)
        darkTheme -> BrandDarkColorScheme
        else -> BrandLightColorScheme
    }
    MaterialExpressiveTheme(
        colorScheme = colorScheme,
        motionScheme = MotionScheme.expressive(),
        typography = ExpressiveTypography,
        content = content,
    )
}
