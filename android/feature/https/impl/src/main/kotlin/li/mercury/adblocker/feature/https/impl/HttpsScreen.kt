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

package li.mercury.adblocker.feature.https.impl

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import li.mercury.adblocker.core.designsystem.component.MarkDefaults
import li.mercury.adblocker.core.designsystem.component.MarkState
import li.mercury.adblocker.core.designsystem.component.StatusMark
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.designsystem.motion.rememberScreenTransitions
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.LevelReason
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.ui.AppIcon
import li.mercury.adblocker.core.ui.AppSnackbarHost
import li.mercury.adblocker.core.ui.FactList
import li.mercury.adblocker.core.ui.FadeThrough
import li.mercury.adblocker.core.ui.MessageEffect
import li.mercury.adblocker.core.ui.Reveal
import li.mercury.adblocker.core.ui.SegmentedGroup
import li.mercury.adblocker.core.ui.SegmentedItem
import li.mercury.adblocker.core.ui.SegmentedToggleItem
import li.mercury.adblocker.core.ui.StepAction
import li.mercury.adblocker.core.ui.StepActions
import li.mercury.adblocker.core.ui.StepPage
import li.mercury.adblocker.core.ui.StepProgress
import li.mercury.adblocker.core.ui.openSecuritySettings
import li.mercury.adblocker.core.ui.profileAppLabel

/** What the certificate is saved as, and the type the system's installer takes. */
private const val CERTIFICATE_NAME = "ADBlocker.crt"
private const val CERTIFICATE_TYPE = "application/x-x509-ca-cert"

@Composable
fun HttpsScreen(onBack: () -> Unit, viewModel: HttpsViewModel, modifier: Modifier = Modifier) {
    val state by viewModel.uiState.collectAsStateWithLifecycle()
    val context = LocalContext.current
    MessageEffect(viewModel.pendingMessages, viewModel::onMessageShown)
    // Back from the system settings: is the certificate there now?
    LifecycleResumeEffect(viewModel) {
        viewModel.checkInstalled()
        onPauseOrDispose {}
    }
    val save = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument(CERTIFICATE_TYPE)) { uri -> uri?.let(viewModel::save) }
    HttpsScreen(
        state = state,
        onBack = onBack,
        onNext = viewModel::next,
        onPrevious = viewModel::previous,
        onSave = { save.launch(CERTIFICATE_NAME) },
        onOpenSettings = {
            viewModel.openingSettings()
            openSecuritySettings(context)
        },
        onTurnOn = viewModel::turnOn,
        modifier = modifier,
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun HttpsScreen(
    state: HttpsUiState,
    onBack: () -> Unit,
    onNext: () -> Unit,
    onPrevious: () -> Unit,
    onSave: () -> Unit,
    onOpenSettings: () -> Unit,
    onTurnOn: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val transitions = rememberScreenTransitions()
    Scaffold(
        modifier = modifier,
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.feature_https_title)) },
                // Each step can be left: what was done stays, and the setup
                // starts again from where it is.
                navigationIcon = {
                    IconButton(onClick = onBack) { Icon(AdbIcons.ArrowBack, contentDescription = stringResource(R.string.feature_https_back)) }
                },
            )
        },
        snackbarHost = { AppSnackbarHost() },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize(), contentAlignment = Alignment.TopCenter) {
            Column(Modifier.widthIn(max = 600.dp).fillMaxSize()) {
                // Folds away with the last step, which is the result rather than a step.
                Reveal(state.step != HttpsStep.Result) {
                    val steps = HttpsStep.entries.size
                    StepProgress(
                        current = state.step.ordinal,
                        count = steps,
                        label = stringResource(R.string.feature_https_step, state.step.ordinal + 1, steps),
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 8.dp),
                    )
                }
                // Steps move along one axis, as screens do.
                AnimatedContent(
                    targetState = state.step,
                    transitionSpec = { if (targetState.ordinal > initialState.ordinal) transitions.forward else transitions.backward },
                    label = "step",
                    modifier = Modifier.weight(1f),
                ) { step ->
                    Column(Modifier.fillMaxSize()) {
                        when (step) {
                            HttpsStep.Impact -> Impact(onLater = onBack, onNext = onNext)
                            HttpsStep.Install -> Install(state, onPrevious, onSave, onOpenSettings)
                            HttpsStep.Confirm -> Confirm(state.busy, onPrevious, onTurnOn)
                            HttpsStep.Result -> Result(state, onDone = onBack)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun ColumnScope.Impact(onLater: () -> Unit, onNext: () -> Unit) {
    StepPage(
        title = stringResource(R.string.feature_https_impact_title),
        text = stringResource(R.string.feature_https_impact_text),
        art = null,
        action = { StepActions(stringResource(R.string.feature_https_later), onLater, stringResource(R.string.feature_https_continue), onNext) },
    ) {
        FactList(
            AdbIcons.Key to R.string.feature_https_impact_key,
            AdbIcons.Public to R.string.feature_https_impact_apps,
            AdbIcons.Info to R.string.feature_https_impact_notice,
            AdbIcons.Delete to R.string.feature_https_impact_remove,
        )
    }
}

@Composable
private fun ColumnScope.Install(
    state: HttpsUiState,
    onPrevious: () -> Unit,
    onSave: () -> Unit,
    onOpenSettings: () -> Unit,
) {
    StepPage(
        title = stringResource(R.string.feature_https_install_title),
        text = stringResource(R.string.feature_https_install_text),
        art = null,
        action = {
            // Saved first: the installer in the settings looks for the file.
            FadeThrough(state, { it.saved }, label = "install action") { state ->
                if (state.saved) {
                    StepActions(stringResource(R.string.feature_https_previous), onPrevious, stringResource(R.string.feature_https_open_security), onOpenSettings)
                } else {
                    StepActions(stringResource(R.string.feature_https_previous), onPrevious, stringResource(R.string.feature_https_save), onSave, mainEnabled = !state.busy)
                }
            }
        },
    ) {
        val rows = listOf(
            Triple(R.string.feature_https_install_save, if (state.saved) R.string.feature_https_install_saved else R.string.feature_https_install_save_text, state.saved),
            Triple(R.string.feature_https_install_open, R.string.feature_https_install_open_text, false),
            Triple(R.string.feature_https_install_choose, R.string.feature_https_install_choose_text, false),
        )
        SegmentedGroup(rows.size) { index ->
            val (title, text, done) = rows[index]
            SegmentedItem(
                index = index,
                count = rows.size,
                headline = stringResource(title),
                supporting = { Text(stringResource(text)) },
                leading = { StepNumber(index + 1, done) },
                onClick = if (index == 0) onSave else null,
            )
        }
        Reveal(state.notFound) {
            Text(
                stringResource(R.string.feature_https_install_not_found),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.error,
                modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 12.dp),
            )
        }
    }
}

/** A numbered instruction, ticked once done. */
@Composable
private fun StepNumber(number: Int, done: Boolean) {
    Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) {
        FadeThrough(done, { it }, contentAlignment = Alignment.Center, label = "step number") { done ->
            if (done) {
                Icon(AdbIcons.CheckCircleFilled, contentDescription = null, Modifier.size(22.dp), tint = MaterialTheme.colorScheme.primary)
            } else {
                Box(Modifier.size(22.dp).clip(CircleShape).background(MaterialTheme.colorScheme.secondaryContainer), contentAlignment = Alignment.Center) {
                    Text(number.toString(), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSecondaryContainer)
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun ColumnScope.Confirm(busy: Boolean, onPrevious: () -> Unit, onTurnOn: () -> Unit) {
    StepPage(
        title = stringResource(R.string.feature_https_apps_title),
        text = stringResource(R.string.feature_https_apps_text),
        art = null,
        action = {
            StepActions(
                stringResource(R.string.feature_https_previous),
                onPrevious,
                stringResource(R.string.feature_https_turn_on),
                onTurnOn,
                mainEnabled = !busy,
            )
        },
    ) {
        Reveal(busy) { Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) { LoadingIndicator() } }
    }
}

@Composable
private fun ColumnScope.Result(state: HttpsUiState, onDone: () -> Unit) {
    StepPage(
        title = stringResource(R.string.feature_https_done_title),
        text = null,
        art = { StatusMark(MarkState.Filtering, size = 128.dp, colors = MarkDefaults.featuredColors()) },
        action = { StepAction(stringResource(R.string.feature_https_finish), onDone) },
        centered = true,
    ) {
        // The apps are read once HTTPS is on: they fade in under the title.
        FadeThrough(state.results, { it::class }, label = "results") { results ->
            Column {
                if (results is ReadState.Unavailable) {
                    Text(
                        stringResource(R.string.feature_https_results_unavailable),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        textAlign = TextAlign.Center,
                        modifier = Modifier.fillMaxWidth().padding(top = 16.dp),
                    )
                }
                ResultRows(results.valueOrNull.orEmpty())
            }
        }
        Text(
            stringResource(R.string.feature_https_done_hint),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
            modifier = Modifier.fillMaxWidth().padding(start = 8.dp, end = 8.dp, top = 12.dp),
        )
    }
}

@Composable
private fun ResultRows(apps: List<AppEntry>) {
    SegmentedGroup(apps.size, Modifier.padding(top = 16.dp)) { index ->
        val entry = apps[index]
        val full = entry.filter?.level == FilterLevel.Full
        val level = when (entry.filter?.levelReason) {
            LevelReason.CaNotInstalled -> R.string.feature_https_result_certificate
            LevelReason.ExclusionsPending -> R.string.feature_https_result_exclusions
            else -> if (full) R.string.feature_https_result_full else R.string.feature_https_result_hosts
        }
        SegmentedItem(
            index = index,
            count = apps.size,
            headline = profileAppLabel(entry.app.label, entry.app.packageName, work = false, entry.app.isSharedGroup),
            supporting = { Text(stringResource(level)) },
            leading = { ResultIcon(full) },
        )
    }
}

@Composable
private fun ResultIcon(full: Boolean) {
    Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) {
        if (full) {
            Icon(AdbIcons.CheckCircleFilled, contentDescription = null, Modifier.size(22.dp), tint = MaterialTheme.colorScheme.primary)
        } else {
            Icon(AdbIcons.Info, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}
