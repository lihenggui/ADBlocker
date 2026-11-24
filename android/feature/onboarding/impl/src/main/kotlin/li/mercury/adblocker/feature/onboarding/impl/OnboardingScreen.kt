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

package li.mercury.adblocker.feature.onboarding.impl

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Checkbox
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.toShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.unit.dp
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import li.mercury.adblocker.core.designsystem.component.MarkDefaults
import li.mercury.adblocker.core.designsystem.component.MarkState
import li.mercury.adblocker.core.designsystem.component.StatusMark
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.designsystem.motion.rememberScreenTransitions
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.ui.FactList
import li.mercury.adblocker.core.ui.Pop
import li.mercury.adblocker.core.ui.StepAction
import li.mercury.adblocker.core.ui.StepPage
import li.mercury.adblocker.core.ui.StepProgress
import li.mercury.adblocker.core.ui.rememberPredictiveBackTransition
import li.mercury.adblocker.feature.onboarding.impl.steps.Apps
import li.mercury.adblocker.feature.onboarding.impl.steps.Check
import li.mercury.adblocker.feature.onboarding.impl.steps.Hint
import li.mercury.adblocker.feature.onboarding.impl.steps.Install
import li.mercury.adblocker.feature.onboarding.impl.steps.Lists
import li.mercury.adblocker.feature.onboarding.impl.steps.StartVpn

@Composable
fun OnboardingScreen(viewModel: OnboardingViewModel, modifier: Modifier = Modifier) {
    val state by viewModel.uiState.collectAsStateWithLifecycle()
    val context = LocalContext.current
    // The system asks for each grant in its own dialog: notifications first,
    // then the VPN, whose refusal leaves filtering off.
    val consent = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        if (result.resultCode == Activity.RESULT_OK) viewModel.startVpn() else viewModel.consentRefused()
    }
    val askConsent = { viewModel.consentIntent()?.let(consent::launch) ?: viewModel.startVpn() }
    val notifications = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        // Either answer goes on: without the notification, the system still shows its VPN icon.
        viewModel.notificationsAnswered(notificationsEnabled(context))
        askConsent()
    }
    OnboardingScreen(
        state = state,
        onNext = viewModel::next,
        onBack = viewModel::back,
        onRetryChecks = viewModel::runChecks,
        onRetryApps = viewModel::loadApps,
        onAcknowledge = viewModel::acknowledge,
        onTogglePick = viewModel::togglePick,
        onFilteringScope = viewModel::setFilteringScope,
        onToggleList = viewModel::toggleList,
        onRetryInstall = viewModel::install,
        onFinish = viewModel::finish,
        modifier = modifier,
        onStartVpn = {
            if (!state.willStartVpn) {
                viewModel.startVpn()
            } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
                ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
            ) {
                notifications.launch(Manifest.permission.POST_NOTIFICATIONS)
            } else {
                viewModel.notificationsAnswered(notificationsEnabled(context))
                askConsent()
            }
        },
    )
}

private fun notificationsEnabled(context: Context) = NotificationManagerCompat.from(context).areNotificationsEnabled()

@Composable
internal fun OnboardingScreen(
    state: OnboardingUiState,
    onNext: () -> Unit,
    onBack: () -> Unit,
    onRetryChecks: () -> Unit,
    onAcknowledge: (Boolean) -> Unit,
    onTogglePick: (String) -> Unit,
    onToggleList: (String) -> Unit,
    onRetryInstall: () -> Unit,
    onFinish: () -> Unit,
    modifier: Modifier = Modifier,
    onFilteringScope: (FilteringScope) -> Unit = {},
    onStartVpn: () -> Unit = {},
    onRetryApps: () -> Unit = {},
) {
    // Keep the install screen in place while filtering starts.
    BackHandler(enabled = state.step != OnboardingStep.Welcome && state.previousStep == null) {}
    val transition = rememberPredictiveBackTransition(state.step, state.previousStep, onBack)
    val transitions = rememberScreenTransitions()
    val vpn = state.method == FilterMethod.Vpn
    Surface(modifier.fillMaxSize(), color = MaterialTheme.colorScheme.surface) {
        transition.AnimatedContent(
            transitionSpec = { if (targetState.ordinal > initialState.ordinal) transitions.forward else transitions.backward },
        ) { step ->
            Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding()) {
                Row(Modifier.fillMaxWidth().height(64.dp).padding(start = 4.dp, end = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.size(48.dp)) {
                        val canGoBack = when (step) {
                            OnboardingStep.Welcome -> false
                            OnboardingStep.Install -> vpn && state.install !is InstallState.Running && state.install !is InstallState.Done
                            else -> true
                        }
                        // Away while installing, back if that fails.
                        Pop(canGoBack) {
                            IconButton(onClick = onBack) {
                                Icon(AdbIcons.ArrowBack, contentDescription = stringResource(R.string.feature_onboarding_back))
                            }
                        }
                    }
                    val steps = OnboardingStep.entries.size
                    StepProgress(
                        current = step.ordinal,
                        count = steps,
                        label = stringResource(R.string.feature_onboarding_step, step.ordinal + 1, steps),
                        modifier = Modifier.weight(1f).padding(start = 4.dp),
                    )
                }
                Box(Modifier.fillMaxWidth().weight(1f), contentAlignment = Alignment.TopCenter) {
                    Column(Modifier.widthIn(max = 600.dp).fillMaxSize()) {
                        when (step) {
                            OnboardingStep.Welcome -> Welcome(onNext)
                            OnboardingStep.Check -> Check(state, onNext, onRetryChecks)
                            OnboardingStep.About -> if (vpn) AboutVpn(onNext) else Certificate(state.acknowledged, onAcknowledge, onNext)
                            OnboardingStep.Apps -> Apps(state, onTogglePick, onFilteringScope, onNext, onRetryApps)
                            OnboardingStep.Lists -> Lists(state, onToggleList, onNext)
                            OnboardingStep.Install -> if (vpn) StartVpn(state, onStartVpn, onFinish) else Install(state, onRetryInstall, onFinish)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun ColumnScope.Welcome(onNext: () -> Unit) {
    StepPage(
        title = stringResource(R.string.feature_onboarding_welcome_title),
        text = stringResource(R.string.feature_onboarding_welcome_text),
        art = { StatusMark(MarkState.Filtering, size = 200.dp, colors = MarkDefaults.featuredColors()) },
        action = { StepAction(stringResource(R.string.feature_onboarding_start), onNext) },
        centered = true,
    ) {
        FactList(
            AdbIcons.Apps to R.string.feature_onboarding_fact_chosen,
            AdbIcons.VisibilityOff to R.string.feature_onboarding_fact_private,
            AdbIcons.LockOpen to R.string.feature_onboarding_fact_methods,
        )
    }
}

/** The step's art: the method's symbol on a cookie shape, in the tertiary container. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun AboutArt(icon: ImageVector) {
    Box(
        Modifier.size(128.dp).clip(MaterialShapes.Cookie9Sided.toShape()).background(MaterialTheme.colorScheme.tertiaryContainer),
        contentAlignment = Alignment.Center,
    ) { Icon(icon, contentDescription = null, tint = MaterialTheme.colorScheme.onTertiaryContainer, modifier = Modifier.size(56.dp)) }
}

@Composable
private fun ColumnScope.Certificate(acknowledged: Boolean, onAcknowledge: (Boolean) -> Unit, onNext: () -> Unit) {
    StepPage(
        title = stringResource(R.string.feature_onboarding_cert_title),
        text = stringResource(R.string.feature_onboarding_cert_text),
        art = { AboutArt(AdbIcons.Key) },
        action = { StepAction(stringResource(R.string.feature_onboarding_continue), onNext, enabled = acknowledged) },
    ) {
        FactList(
            AdbIcons.Key to R.string.feature_onboarding_cert_key,
            AdbIcons.Info to R.string.feature_onboarding_cert_notice,
            AdbIcons.Public to R.string.feature_onboarding_cert_browsers,
        )
        Row(
            Modifier.fillMaxWidth().padding(top = 12.dp).heightIn(min = 56.dp).clip(RoundedCornerShape(16.dp))
                .toggleable(value = acknowledged, role = Role.Checkbox, onValueChange = onAcknowledge)
                .padding(horizontal = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Checkbox(checked = acknowledged, onCheckedChange = null, modifier = Modifier.padding(12.dp))
            Text(stringResource(R.string.feature_onboarding_cert_ack), style = MaterialTheme.typography.bodyLarge)
        }
    }
}

/**
 * What the VPN does and costs. Nothing is decrypted until the user installs
 * the certificate, so there is nothing to acknowledge: the HTTPS setup,
 * which needs it, comes later.
 */
@Composable
private fun ColumnScope.AboutVpn(onNext: () -> Unit) {
    StepPage(
        title = stringResource(R.string.feature_onboarding_vpn_title),
        text = stringResource(R.string.feature_onboarding_vpn_text),
        art = { AboutArt(AdbIcons.VpnKey) },
        action = { StepAction(stringResource(R.string.feature_onboarding_continue), onNext) },
    ) {
        FactList(
            AdbIcons.Apps to R.string.feature_onboarding_vpn_apps,
            AdbIcons.Dns to R.string.feature_onboarding_vpn_domains,
            AdbIcons.VpnLock to R.string.feature_onboarding_vpn_one,
            AdbIcons.CloudOff to R.string.feature_onboarding_vpn_local,
        )
        Hint(stringResource(R.string.feature_onboarding_vpn_hint))
    }
}
