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

import androidx.compose.material3.Typography
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.sp

/*
 * The type scale with its emphasized styles. MaterialExpressiveTheme's own
 * default calls the constructor that sets each emphasized style to its plain
 * one, so "Display large emphasized" would be drawn at weight 400. The
 * weights and tracking below are the M3 Expressive tokens
 * (TypeScaleTokens.*Emphasized*, material3 1.5): medium for display,
 * headline, title large and body; bold for title medium and small and for the
 * labels. Sizes and line heights are those of the plain styles.
 */

private fun TextStyle.emphasized(weight: FontWeight, tracking: TextUnit): TextStyle = copy(fontWeight = weight, letterSpacing = tracking)

internal val ExpressiveTypography: Typography = Typography().let { base ->
    Typography(
        displayLarge = base.displayLarge,
        displayMedium = base.displayMedium,
        displaySmall = base.displaySmall,
        headlineLarge = base.headlineLarge,
        headlineMedium = base.headlineMedium,
        headlineSmall = base.headlineSmall,
        titleLarge = base.titleLarge,
        titleMedium = base.titleMedium,
        titleSmall = base.titleSmall,
        bodyLarge = base.bodyLarge,
        bodyMedium = base.bodyMedium,
        bodySmall = base.bodySmall,
        labelLarge = base.labelLarge,
        labelMedium = base.labelMedium,
        labelSmall = base.labelSmall,
        displayLargeEmphasized = base.displayLarge.emphasized(FontWeight.Medium, 0.sp),
        displayMediumEmphasized = base.displayMedium.emphasized(FontWeight.Medium, 0.sp),
        displaySmallEmphasized = base.displaySmall.emphasized(FontWeight.Medium, 0.sp),
        headlineLargeEmphasized = base.headlineLarge.emphasized(FontWeight.Medium, 0.sp),
        headlineMediumEmphasized = base.headlineMedium.emphasized(FontWeight.Medium, 0.sp),
        headlineSmallEmphasized = base.headlineSmall.emphasized(FontWeight.Medium, 0.sp),
        titleLargeEmphasized = base.titleLarge.emphasized(FontWeight.Medium, 0.sp),
        titleMediumEmphasized = base.titleMedium.emphasized(FontWeight.Bold, 0.15.sp),
        titleSmallEmphasized = base.titleSmall.emphasized(FontWeight.Bold, 0.1.sp),
        bodyLargeEmphasized = base.bodyLarge.emphasized(FontWeight.Medium, 0.15.sp),
        bodyMediumEmphasized = base.bodyMedium.emphasized(FontWeight.Medium, 0.25.sp),
        bodySmallEmphasized = base.bodySmall.emphasized(FontWeight.Medium, 0.4.sp),
        labelLargeEmphasized = base.labelLarge.emphasized(FontWeight.Bold, 0.1.sp),
        labelMediumEmphasized = base.labelMedium.emphasized(FontWeight.Bold, 0.5.sp),
        labelSmallEmphasized = base.labelSmall.emphasized(FontWeight.Bold, 0.5.sp),
    )
}
