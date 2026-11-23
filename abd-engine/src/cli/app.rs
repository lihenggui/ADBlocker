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

use std::io::{Read, Write};

use anyhow::{Result, bail};

use super::{Ctx, Host, changed, print_json, unknown};
use crate::apps::{self, Change, Configure};
use crate::pause::Pauses;
use crate::{clock, files, protocol, settings};

pub(super) fn run(
    ctx: &Ctx<'_>,
    args: &[&str],
    input: &mut dyn Read,
    out: &mut dyn Write,
) -> Result<()> {
    let Ctx { paths, host, .. } = *ctx;
    match args {
        ["app", "list"] => {
            for app in listed_apps(ctx)? {
                if let Some(serial) = app.profile_serial {
                    writeln!(out, "{serial}:{}", app.package)?;
                } else {
                    writeln!(out, "{}", app.package)?;
                }
            }
            Ok(())
        }
        ["app", "list", "--json"] => print_json(out, &listed_apps(ctx)?),
        ["app", "selections", "--json"] => {
            let text = files::read_text(&paths.apps(), apps::MAX_FILE)?;
            print_json(out, &protocol::app_selections(&text, ctx.settings()?.scope))
        }
        ["app", "configure-exclusions"] => {
            let selection = files::read_limited(input, apps::MAX_FILE)?;
            apps::configure_exclusions(&paths.apps(), &selection)?;
            changed(host)
        }
        ["app", action @ ("exclude" | "include"), name] => {
            apps::set_excluded(&paths.apps(), name, *action == "exclude")?;
            if *action == "exclude" {
                Pauses::change(&paths.pauses(), clock::now(), |p| p.resume_app(name))?;
            }
            changed(host)
        }
        ["app", "configure"] => {
            let selection = files::read_limited(input, apps::MAX_FILE)?;
            apps::configure(
                &paths.apps(),
                &selection,
                Configure::Replace,
                |apps, added| host.validate_selection(apps, added),
            )?;
            changed(host)
        }
        ["app", "add", name] => {
            apps::add(&paths.apps(), name, |apps, added| {
                host.validate_selection(apps, added)
            })?;
            changed(host)
        }
        ["app", "remove", name] => {
            apps::edit(&paths.apps(), name, Change::Remove)?;
            Pauses::change(&paths.pauses(), clock::now(), |p| p.resume_app(name))?;
            changed(host)
        }
        _ => unknown(args),
    }
}

fn listed_apps(ctx: &Ctx<'_>) -> Result<Vec<protocol::App>> {
    let Ctx { paths, host, .. } = *ctx;
    let settings = ctx.settings()?;
    let text = files::read_text(&paths.apps(), apps::MAX_FILE)?;
    let selected = scoped_apps(&text, settings.scope, host);
    let names: Vec<_> = selected.iter().map(String::as_str).collect();
    let mut apps = protocol::app_levels(
        paths,
        &names,
        settings,
        host.undecryptable(),
        &host.trusted_profiles(),
        clock::now(),
    );
    host.report_app_eligibility(&mut apps);
    Ok(apps)
}

fn scoped_apps(text: &str, scope: settings::Scope, host: &dyn Host) -> Vec<String> {
    apps::in_scope(
        text,
        scope,
        |saved| saved.into_iter().map(str::to_owned).collect::<Vec<_>>(),
        |selected, exclusions| selected.extend(host.personal_app_identities(exclusions)),
    )
}

/// Fails unless `package` is one of the filtered apps.
pub(super) fn chosen(ctx: &Ctx<'_>, package: &str) -> Result<()> {
    let Ctx { paths, host, .. } = *ctx;
    let text = files::read_text(&paths.apps(), apps::MAX_FILE)?;
    let selected = apps::in_scope(
        &text,
        ctx.settings()?.scope,
        |saved| saved.contains(&package),
        |selected, exclusions| {
            if apps::Identity::parse(package).is_some_and(|id| id.profile_serial.is_none()) {
                *selected = host
                    .personal_app_identities(exclusions)
                    .iter()
                    .any(|name| name == package);
            }
        },
    );
    if !selected {
        bail!("not a filtered app: {package}");
    }
    Ok(())
}
