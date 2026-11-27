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

//! Policy routes that bring marked TCP packets to the loopback interface.

use crate::capture::MARK_CAPTURE;

use super::{Family, PREF, TABLE, Tools, mark};

pub(super) fn arm(tools: &mut dyn Tools, family: Family) -> anyhow::Result<()> {
    let ip = family.ip();
    let code = tools.run(
        "ip",
        &[
            ip, "route", "replace", "local", "default", "dev", "lo", "table", TABLE,
        ],
        "",
    )?;
    anyhow::ensure!(code == 0, "ip {ip} route replace failed ({code})");
    let capture = mark(MARK_CAPTURE);
    let code = tools.run(
        "ip",
        &[
            ip, "rule", "add", "fwmark", &capture, "lookup", TABLE, "pref", PREF,
        ],
        "",
    )?;
    anyhow::ensure!(code == 0, "ip {ip} rule add failed ({code})");
    Ok(())
}

/// A failed deletion may mean that the rule was already gone. Confirm
/// that distinction before changing selectors or rearming the gate.
fn disarm(tools: &mut dyn Tools, family: Family) -> anyhow::Result<()> {
    let capture = mark(MARK_CAPTURE);
    let args = [
        family.ip(),
        "rule",
        "del",
        "fwmark",
        &capture,
        "lookup",
        TABLE,
        "pref",
        PREF,
    ];
    for _ in 0..8 {
        if !matches!(tools.run("ip", &args, ""), Ok(0)) {
            break;
        }
    }
    let result = (|| {
        anyhow::ensure!(
            !tools.route_present(family)?,
            "could not remove the {} capture routing rule",
            family.ip()
        );
        Ok(())
    })();
    if result.is_err() {
        // The route itself is an independent way of disabling the gate.
        // Keep the error so nobody publishes active capture or rearms it.
        remove_route(tools, family);
    }
    result
}

pub(super) fn disarm_all(tools: &mut dyn Tools) -> anyhow::Result<()> {
    let gate = tools.set_gate(false);
    let results = Family::BOTH.map(|family| disarm(tools, family));
    gate.and(results.into_iter().collect())
}

pub(super) fn remove_route(tools: &mut dyn Tools, family: Family) {
    // If there is no route, the tool reports an error.
    let _ = tools.run(
        "ip",
        &[
            family.ip(),
            "route",
            "del",
            "local",
            "default",
            "dev",
            "lo",
            "table",
            TABLE,
        ],
        "",
    );
}
