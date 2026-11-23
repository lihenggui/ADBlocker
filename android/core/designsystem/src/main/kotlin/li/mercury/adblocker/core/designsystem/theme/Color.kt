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

import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.ui.graphics.Color

/*
 * The app's own color scheme, used when the user turns off wallpaper colors.
 * Tonal palettes as Material Theme Builder makes them from the launcher icon:
 * the Android green of its eyes (#3DDC84) as the primary source and the navy
 * of its face, Blocker's stripe color (#073042), as the tertiary source. Tones
 * follow the Material 3 role mapping (primary is tone 40 in light and 80 in
 * dark). The error roles are the baseline ones, which dynamic color keeps too.
 */

internal val BrandLightColorScheme = lightColorScheme(
    primary = Color(0xFF1F6B45),
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFFA8F2C4),
    onPrimaryContainer = Color(0xFF005232),
    inversePrimary = Color(0xFF8DD5A9),
    secondary = Color(0xFF4D6356),
    onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFCFE9D8),
    onSecondaryContainer = Color(0xFF364B3F),
    tertiary = Color(0xFF2E6385),
    onTertiary = Color(0xFFFFFFFF),
    tertiaryContainer = Color(0xFFC6E7FF),
    onTertiaryContainer = Color(0xFF0E4B6A),
    background = Color(0xFFF5FBF5),
    onBackground = Color(0xFF171D1A),
    surface = Color(0xFFF5FBF5),
    onSurface = Color(0xFF171D1A),
    surfaceVariant = Color(0xFFDCE5DD),
    onSurfaceVariant = Color(0xFF404943),
    surfaceTint = Color(0xFF1F6B45),
    inverseSurface = Color(0xFF2C322E),
    inverseOnSurface = Color(0xFFECF2EC),
    error = Color(0xFFBA1A1A),
    onError = Color(0xFFFFFFFF),
    errorContainer = Color(0xFFFFDAD6),
    onErrorContainer = Color(0xFF93000A),
    outline = Color(0xFF707972),
    outlineVariant = Color(0xFFC0C9C1),
    scrim = Color(0xFF000000),
    surfaceBright = Color(0xFFF5FBF5),
    surfaceDim = Color(0xFFD6DBD6),
    surfaceContainerLowest = Color(0xFFFFFFFF),
    surfaceContainerLow = Color(0xFFEFF5EF),
    surfaceContainer = Color(0xFFE9F0E9),
    surfaceContainerHigh = Color(0xFFE4EAE4),
    surfaceContainerHighest = Color(0xFFDEE4DE),
)

internal val BrandDarkColorScheme = darkColorScheme(
    primary = Color(0xFF8DD5A9),
    onPrimary = Color(0xFF00391F),
    primaryContainer = Color(0xFF005232),
    onPrimaryContainer = Color(0xFFA8F2C4),
    inversePrimary = Color(0xFF1F6B45),
    secondary = Color(0xFFB3CCBC),
    onSecondary = Color(0xFF1F352A),
    secondaryContainer = Color(0xFF364B3F),
    onSecondaryContainer = Color(0xFFCFE9D8),
    tertiary = Color(0xFFA3CBE6),
    onTertiary = Color(0xFF00344F),
    tertiaryContainer = Color(0xFF1C4B68),
    onTertiaryContainer = Color(0xFFC6E7FF),
    background = Color(0xFF0F1511),
    onBackground = Color(0xFFDEE4DE),
    surface = Color(0xFF0F1511),
    onSurface = Color(0xFFDEE4DE),
    surfaceVariant = Color(0xFF404943),
    onSurfaceVariant = Color(0xFFC0C9C1),
    surfaceTint = Color(0xFF8DD5A9),
    inverseSurface = Color(0xFFDEE4DE),
    inverseOnSurface = Color(0xFF2C322E),
    error = Color(0xFFFFB4AB),
    onError = Color(0xFF690005),
    errorContainer = Color(0xFF93000A),
    onErrorContainer = Color(0xFFFFDAD6),
    outline = Color(0xFF8A938B),
    outlineVariant = Color(0xFF404943),
    scrim = Color(0xFF000000),
    surfaceBright = Color(0xFF353B37),
    surfaceDim = Color(0xFF0F1511),
    surfaceContainerLowest = Color(0xFF0A0F0C),
    surfaceContainerLow = Color(0xFF171D1A),
    surfaceContainer = Color(0xFF1B211D),
    surfaceContainerHigh = Color(0xFF252B28),
    surfaceContainerHighest = Color(0xFF303632),
)
