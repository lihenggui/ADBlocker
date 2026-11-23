// Copyright (C) 2026 Mercury Li
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License version 3 as
// published by the Free Software Foundation.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

use std::sync::{Arc, RwLock};

use abd_engine::{filter, mitm, pins};
use anyhow::{Context as _, Result};
use tokio::task::{JoinError, JoinSet};

use super::write_state;
use crate::paths::Paths;

pub(super) type Compiled = (u64, Result<filter::Filters>);

/// Called both for a reload request and when the worker finishes. Recheck
/// the files then, so even several edits during a build need only one worker.
pub(super) fn refresh_filters(
    jobs: &mut tokio::task::JoinSet<Compiled>,
    active: &RwLock<Arc<filter::Filters>>,
    paths: &Paths,
    verbose: bool,
    finished: Option<Compiled>,
) -> Result<bool> {
    if !jobs.is_empty() {
        return Ok(false);
    }
    let files = filter::ListFiles::read(&[&paths.lists(), &paths.rules()])?;
    if let Some((generation, result)) = finished
        && files.generation == generation
    {
        *active.write().unwrap() = Arc::new(result?);
        return Ok(true);
    }
    if Some(files.generation) != active.read().unwrap().generation {
        let cache = paths.cache();
        jobs.spawn_blocking(move || (files.generation, files.filters(Some(&cache), verbose)));
    }
    Ok(false)
}

pub(super) fn on_signal(jobs: &mut JoinSet<Compiled>, ctx: &mitm::Ctx, paths: &Paths) {
    if let Err(e) = refresh_filters(jobs, &ctx.lists.filters, paths, ctx.hooks.verbose, None) {
        eprintln!("lists: kept as they were: {e}");
    }
    ctx.reload_sources(paths, |policy, forget| {
        tokio::task::block_in_place(|| pins::take_requests(policy, forget))
    });
}

pub(super) fn on_compiled(
    jobs: &mut JoinSet<Compiled>,
    ctx: &mitm::Ctx,
    paths: &Paths,
    v6: bool,
    result: Result<Compiled, JoinError>,
) -> Result<()> {
    let compiled = result.context("filter compiler task failed")?;
    match refresh_filters(
        jobs,
        &ctx.lists.filters,
        paths,
        ctx.hooks.verbose,
        Some(compiled),
    ) {
        Ok(true) => write_state(ctx, paths, v6),
        Err(e) => eprintln!("lists: kept as they were: {e}"),
        Ok(false) => {}
    }
    Ok(())
}
