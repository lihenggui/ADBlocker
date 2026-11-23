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

package li.mercury.adblocker.sync.work.di

import dagger.Binds
import dagger.Module
import dagger.hilt.InstallIn
import dagger.hilt.components.SingletonComponent
import li.mercury.adblocker.core.data.repository.ListUpdateScheduler
import li.mercury.adblocker.sync.work.UpdateScheduler

@Module
@InstallIn(SingletonComponent::class)
internal abstract class SyncModule {
    @Binds abstract fun listUpdateScheduler(impl: UpdateScheduler): ListUpdateScheduler
}
