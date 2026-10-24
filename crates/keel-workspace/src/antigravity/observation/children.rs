//! Child identities outlive the bounded usage replay. Discover historical invocations once,
//! then inspect only new tool rows and unsettled rows whose result may still name a child.
use super::super::{CANCELLED, DONE, FAILED, SETTLED, TOOL, children, finished, line, subagent};
use camino::Utf8Path;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default, Debug)]
/// Child identity history belongs to this transcript reader, so recent usage truncation cannot
/// silently turn a long-running child idle or prevent its later restart from being observed.
pub(super) struct Reader {
    cursor: u64,
    pending: BTreeSet<i64>,
    known: BTreeMap<String, Option<String>>,
}

impl Reader {
    /// Discover only newly written or unfinished invocations, then recheck known child activity.
    /// A fresh reader also discovers earlier launches so reconnects reconstruct current work.
    pub(super) fn background(
        &mut self,
        c: &rusqlite::Connection,
        path: &Utf8Path,
        total: u64,
    ) -> Option<BTreeMap<String, Option<String>>> {
        if total < self.cursor {
            *self = Self::default();
        }
        let pending: Vec<_> = self.pending.iter().copied().collect();
        let mut q = c.prepare("SELECT idx, status, step_payload FROM steps WHERE idx >= ?1 AND step_type = ?2 AND (instr(step_payload, ?3) > 0 OR status NOT IN (?4,?5,?6)) ORDER BY idx").ok()?;
        let rows = q
            .query_map(
                rusqlite::params![
                    self.cursor as i64,
                    TOOL,
                    b"invoke_subagent".as_slice(),
                    DONE,
                    CANCELLED,
                    FAILED
                ],
                row,
            )
            .ok()?;
        for result in rows {
            let (idx, status, payload) = result.ok()?;
            self.note(idx, status, &payload);
        }
        for idx in pending {
            match c.query_row(
                "SELECT idx, status, step_payload FROM steps WHERE idx=?1",
                [idx],
                row,
            ) {
                Ok((idx, status, payload)) => self.note(idx, status, &payload),
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    self.pending.remove(&idx);
                }
                Err(_) => return None,
            }
        }
        self.cursor = total;
        Some(
            self.known
                .iter()
                .filter(|(id, _)| !finished(path, id))
                .map(|(id, agent)| (id.clone(), agent.clone()))
                .collect(),
        )
    }

    fn note(&mut self, idx: i64, status: i64, payload: &[u8]) {
        if SETTLED.contains(&status) {
            self.pending.remove(&idx);
        } else {
            self.pending.insert(idx);
        }
        let v = line(idx, TOOL, status, payload);
        if v["call"]["name"] != "invoke_subagent" {
            return;
        }
        for (i, id) in children(v["output"].as_str().unwrap_or_default())
            .into_iter()
            .enumerate()
        {
            let agent = subagent(&v["call"]["args"], i)["TypeName"]
                .as_str()
                .map(str::to_string);
            self.known.insert(id, agent);
        }
    }
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<(i64, i64, Vec<u8>)> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get::<_, Option<Vec<u8>>>(2)?.unwrap_or_default(),
    ))
}
