//! Visual workflow builder: event-triggered automations from a small, fixed catalogue of safe nodes.
//!
//! A workflow is a versioned graph: one trigger (an event type from `events::TYPES`, optionally a subject) and
//! up to 20 nodes joined by edges. Node types are deliberately limited:
//!  * `condition`  - compare a field of the event payload; false stops that branch;
//!  * `notify`     - an in-app notification;
//!  * `pause_bot`  - pause a running bot (never starts, resumes or arms anything);
//!  * `request_approval` - open an approval request; a person still decides;
//!  * `risk_assessment`  - snapshot the portfolio risk view into the run log;
//!  * `start_research`   - queue a bounded research job (see `researcher`).
//! There is no node that places an order, changes a limit, arms live trading or touches a credential, and graph
//! validation refuses any such type by name. So a workflow cannot bypass risk: it has no order path at all.
//!
//! Validation: known trigger, known node types, valid params, edges between existing nodes, every node
//! reachable, no cycles. Dry-run evaluates the graph against an event and returns what would happen without
//! doing it. Live runs are idempotent per (workflow, triggering event), limited to 30 per hour per workflow,
//! and events emitted by workflows never trigger workflows (no loops).

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub const NODE_TYPES: [&str; 6] = ["condition", "notify", "pause_bot", "request_approval", "risk_assessment", "start_research"];
const FORBIDDEN: [&str; 7] = ["place_order", "open_position", "close_position", "change_limits", "arm_live", "withdraw", "set_credentials"];
const OPS: [&str; 6] = ["eq", "ne", "gt", "lt", "contains", "exists"];
pub const MAX_NODES: usize = 20;
const RUNS_PER_HOUR: i64 = 30;
/// Workflow-originated events never trigger workflows.
const SELF_EVENTS: [&str; 2] = ["WorkflowTriggered", "WorkflowActionCompleted"];

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Every problem with the graph (empty when valid).
pub fn validate(g: &Value) -> Vec<String> {
    let mut e = vec![];
    let trig = g["trigger"]["event"].as_str().unwrap_or("");
    if !crate::events::TYPES.contains(&trig) {
        e.push(format!("trigger.event must be one of the event types (got {trig:?})"));
    } else if SELF_EVENTS.contains(&trig) {
        e.push("a workflow cannot be triggered by workflow events".into());
    }
    let nodes = g["nodes"].as_array().cloned().unwrap_or_default();
    if nodes.is_empty() {
        e.push("add at least one node".into());
    }
    if nodes.len() > MAX_NODES {
        e.push(format!("at most {MAX_NODES} nodes"));
    }
    let mut ids = BTreeSet::new();
    for n in &nodes {
        let id = n["id"].as_str().unwrap_or("");
        let ty = n["type"].as_str().unwrap_or("");
        if id.is_empty() || id == "trigger" || id.len() > 40 || !ids.insert(id.to_string()) {
            e.push(format!("node ids must be unique, non-empty and not \"trigger\" ({id:?})"));
        }
        if FORBIDDEN.contains(&ty) {
            e.push(format!("node {id}: {ty} is not allowed in workflows (orders, limits, live arming and credentials are never automated here)"));
            continue;
        }
        if !NODE_TYPES.contains(&ty) {
            e.push(format!("node {id}: unknown type {ty:?}"));
            continue;
        }
        let p = &n["params"];
        match ty {
            "condition" => {
                if p["field"].as_str().map_or(true, str::is_empty) || !OPS.contains(&p["op"].as_str().unwrap_or("")) {
                    e.push(format!("node {id}: condition needs a field and op ({})", OPS.join(", ")));
                }
            }
            "notify" => {
                if p["title"].as_str().map_or(true, |t| t.trim().is_empty() || t.len() > 200) {
                    e.push(format!("node {id}: notify needs a title (up to 200 chars)"));
                }
            }
            "pause_bot" | "request_approval" => {
                if p["bot_id"].as_i64().is_none() && p["bot_id"] != "from_event" {
                    e.push(format!("node {id}: bot_id must be a bot id or \"from_event\""));
                }
            }
            "start_research" => {
                if p["symbol"].as_str().map_or(true, str::is_empty) {
                    e.push(format!("node {id}: start_research needs a symbol"));
                }
            }
            _ => {}
        }
    }
    let edges = g["edges"].as_array().cloned().unwrap_or_default();
    if edges.len() > MAX_NODES * 2 {
        e.push("too many edges".into());
    }
    let mut adj: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for ed in &edges {
        let (a, b) = (ed[0].as_str().unwrap_or(""), ed[1].as_str().unwrap_or(""));
        if !(a == "trigger" || ids.contains(a)) || !ids.contains(b) {
            e.push(format!("edge {a} -> {b} joins unknown nodes"));
            continue;
        }
        adj.entry(a.into()).or_default().push(b.into());
    }
    // cycles (DFS colouring) and reachability from the trigger
    fn dfs(n: &str, adj: &BTreeMap<String, Vec<String>>, colour: &mut BTreeMap<String, u8>) -> bool {
        colour.insert(n.into(), 1);
        for m in adj.get(n).into_iter().flatten() {
            match colour.get(m.as_str()).copied().unwrap_or(0) {
                1 => return true,
                0 if dfs(m, adj, colour) => return true,
                _ => {}
            }
        }
        colour.insert(n.into(), 2);
        false
    }
    let mut colour = BTreeMap::new();
    if dfs("trigger", &adj, &mut colour) || ids.iter().any(|i| colour.get(i.as_str()).is_none() && dfs(i, &adj, &mut colour)) {
        e.push("the graph has a cycle".into());
    }
    let mut seen = BTreeSet::new();
    let mut stack = vec!["trigger".to_string()];
    while let Some(n) = stack.pop() {
        for m in adj.get(&n).into_iter().flatten() {
            if seen.insert(m.clone()) {
                stack.push(m.clone());
            }
        }
    }
    for i in &ids {
        if !seen.contains(i) {
            e.push(format!("node {i} is not reachable from the trigger"));
        }
    }
    e
}

fn field<'a>(ev: &'a Value, path: &str) -> &'a Value {
    path.split('.').fold(ev, |v, k| &v[k])
}

fn test(cond: &Value, ev: &Value) -> bool {
    let v = field(ev, cond["field"].as_str().unwrap_or(""));
    let want = &cond["value"];
    let num = |x: &Value| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok()));
    match cond["op"].as_str().unwrap_or("") {
        "exists" => !v.is_null(),
        "eq" => v == want || (num(v).is_some() && num(v) == num(want)),
        "ne" => !(v == want || (num(v).is_some() && num(v) == num(want))),
        "gt" => matches!((num(v), num(want)), (Some(a), Some(b)) if a > b),
        "lt" => matches!((num(v), num(want)), (Some(a), Some(b)) if a < b),
        "contains" => v.to_string().to_lowercase().contains(&want.as_str().unwrap_or("").to_lowercase()),
        _ => false,
    }
}

fn bot_of(p: &Value, ev: &Value) -> Option<i64> {
    if p["bot_id"] == "from_event" {
        ev["payload"]["bot_id"].as_i64().or_else(|| ev["subject"].as_str().and_then(|s| s.trim_start_matches("bot:").parse().ok()))
    } else {
        p["bot_id"].as_i64()
    }
}

/// Walk the graph for one event. With `dry_run` nothing is changed. Returns the steps and any research
/// requests (the caller queues those as jobs).
pub fn execute(conn: &Connection, g: &Value, ev: &Value, dry_run: bool) -> (Vec<Value>, Vec<Value>) {
    let nodes: BTreeMap<String, Value> = g["nodes"].as_array().into_iter().flatten().filter_map(|n| Some((n["id"].as_str()?.to_string(), n.clone()))).collect();
    let mut adj: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for ed in g["edges"].as_array().into_iter().flatten() {
        adj.entry(ed[0].as_str().unwrap_or("").into()).or_default().push(ed[1].as_str().unwrap_or("").into());
    }
    let (mut steps, mut research) = (vec![], vec![]);
    let mut queue: std::collections::VecDeque<String> = adj.get("trigger").cloned().unwrap_or_default().into();
    let mut done = BTreeSet::new();
    while let Some(id) = queue.pop_front() {
        if !done.insert(id.clone()) {
            continue;
        }
        let Some(n) = nodes.get(&id) else { continue };
        let p = &n["params"];
        let ty = n["type"].as_str().unwrap_or("");
        let (follow, outcome): (bool, Value) = match ty {
            "condition" => {
                let ok = test(p, ev);
                (ok, json!(if ok { "true" } else { "false: branch stopped" }))
            }
            "notify" => {
                let title = p["title"].as_str().unwrap_or("");
                if !dry_run {
                    crate::events::notify(conn, "workflow", title, &format!("{} event #{}", ev["type"].as_str().unwrap_or(""), ev["id"]), Some(&format!("wf:{}:{id}", ev["id"])));
                }
                (true, json!(format!("notify \"{title}\"")))
            }
            "pause_bot" => match bot_of(p, ev) {
                Some(b) => {
                    let status: Option<String> = conn.query_row("SELECT status FROM bots WHERE id = ?1", [b], |r| r.get(0)).optional().ok().flatten();
                    match status.as_deref() {
                        Some("running") if dry_run => (true, json!(format!("would pause bot #{b}"))),
                        Some("running") => match crate::bots::set_status(conn, b, "paused", "paused by a workflow", "workflow") {
                            Ok(()) => (true, json!(format!("paused bot #{b}"))),
                            Err(e) => (false, json!(format!("could not pause bot #{b}: {e:?}"))),
                        },
                        Some(s) => (true, json!(format!("bot #{b} is {s}: nothing to pause"))),
                        None => (false, json!(format!("bot #{b} does not exist"))),
                    }
                }
                None => (false, json!("no bot id in the event")),
            },
            "request_approval" => match bot_of(p, ev) {
                Some(b) if dry_run => (true, json!(format!("would request an approval review for bot #{b} (a person decides)"))),
                Some(b) => match crate::approvals::request(conn, b, "workflow") {
                    Ok(a) => (true, json!(format!("approval #{} requested; a person must decide", a["id"]))),
                    Err(e) => (false, json!(format!("approval not requested: {e}"))),
                },
                None => (false, json!("no bot id in the event")),
            },
            "risk_assessment" => {
                let v = crate::riskintel::view(conn);
                (true, json!({ "risk_snapshot": v }))
            }
            "start_research" => {
                let h = json!({ "symbol": p["symbol"], "interval": p["interval"].as_str().unwrap_or("1h"), "templates": p["templates"], "objective": p["objective"].as_str().unwrap_or("return") });
                if !dry_run {
                    research.push(h.clone());
                }
                (true, json!(format!("{} a research run on {}", if dry_run { "would queue" } else { "queued" }, p["symbol"].as_str().unwrap_or(""))))
            }
            _ => (false, json!("unknown node")),
        };
        if !dry_run && ty != "condition" {
            crate::events::emit(conn, "WorkflowActionCompleted", &format!("node:{id}"), json!({ "node": id, "type": ty, "outcome": outcome }), ev["id"].as_i64().map(|i| format!("event:{i}")).as_deref(), None);
        }
        steps.push(json!({ "node": id, "type": ty, "outcome": outcome, "dry_run": dry_run }));
        if follow {
            queue.extend(adj.get(&id).cloned().unwrap_or_default());
        }
    }
    (steps, research)
}

fn load(conn: &Connection, id: i64) -> Result<(String, String, i64, Value), String> {
    let (name, status, ver): (String, String, i64) = conn.query_row("SELECT name, status, version FROM workflows WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(|_| "no such workflow".to_string())?;
    let g: String = conn.query_row("SELECT graph FROM workflow_versions WHERE workflow_id = ?1 AND version = ?2", params![id, ver], |r| r.get(0)).map_err(|e| e.to_string())?;
    Ok((name, status, ver, serde_json::from_str(&g).unwrap_or(Value::Null)))
}

pub fn get(conn: &Connection, id: i64) -> Result<Value, String> {
    let (name, status, ver, g) = load(conn, id)?;
    let runs: Vec<Value> = conn
        .prepare("SELECT id, version, trigger_event_id, dry_run, status, steps, error, created_utc FROM workflow_runs WHERE workflow_id = ?1 ORDER BY id DESC LIMIT 20")
        .and_then(|mut st| {
            st.query_map([id], |r| Ok(json!({ "id": r.get::<_, i64>(0)?, "version": r.get::<_, i64>(1)?, "trigger_event_id": r.get::<_, Option<i64>>(2)?, "dry_run": r.get::<_, i64>(3)? != 0, "status": r.get::<_, String>(4)?, "steps": serde_json::from_str::<Value>(&r.get::<_, String>(5)?).unwrap_or(Value::Null), "error": r.get::<_, Option<String>>(6)?, "created_utc": r.get::<_, String>(7)? })))
                .map(|m| m.filter_map(Result::ok).collect())
        })
        .unwrap_or_default();
    Ok(json!({ "id": id, "name": name, "status": status, "version": ver, "graph": g, "problems": validate(&g), "runs": runs }))
}

pub fn list(conn: &Connection) -> Result<Value, String> {
    let ids: Vec<i64> = conn.prepare("SELECT id FROM workflows WHERE status <> 'archived' ORDER BY id").and_then(|mut st| st.query_map([], |r| r.get(0)).map(|m| m.filter_map(Result::ok).collect())).map_err(|e| e.to_string())?;
    let items: Vec<Value> = ids.into_iter().filter_map(|i| get(conn, i).ok()).map(|mut w| {
        w.as_object_mut().map(|o| o.remove("runs"));
        w
    }).collect();
    Ok(json!({ "workflows": items, "node_types": NODE_TYPES, "trigger_events": crate::events::TYPES.iter().filter(|t| !SELF_EVENTS.contains(t)).collect::<Vec<_>>() }))
}

/// Create (`id` None) or save a new version. A saved version is `validated` when it has no problems, else `draft`;
/// either way an enabled workflow must be enabled again after an edit.
pub fn save(conn: &Connection, id: Option<i64>, name: &str, g: &Value) -> Result<Value, String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 80 {
        return Err("name is required (up to 80 chars)".into());
    }
    if g.to_string().len() > 32_000 {
        return Err("graph is too large".into());
    }
    let problems = validate(g);
    let status = if problems.is_empty() { "validated" } else { "draft" };
    let t = now();
    let id = match id {
        None => {
            conn.execute("INSERT INTO workflows (name, status, version, created_utc, updated_utc) VALUES (?1,?2,0,?3,?3)", params![name, status, t]).map_err(|e| e.to_string())?;
            conn.last_insert_rowid()
        }
        Some(i) => {
            conn.query_row("SELECT 1 FROM workflows WHERE id = ?1", [i], |_| Ok(())).map_err(|_| "no such workflow".to_string())?;
            i
        }
    };
    let ver: i64 = conn.query_row("SELECT version + 1 FROM workflows WHERE id = ?1", [id], |r| r.get(0)).map_err(|e| e.to_string())?;
    conn.execute("INSERT INTO workflow_versions (workflow_id, version, graph, created_utc) VALUES (?1,?2,?3,?4)", params![id, ver, g.to_string(), t]).map_err(|e| e.to_string())?;
    conn.execute("UPDATE workflows SET name = ?2, status = ?3, version = ?4, updated_utc = ?5 WHERE id = ?1", params![id, name, status, ver, t]).map_err(|e| e.to_string())?;
    let _ = crate::audit::record(conn, "user", "workflow_saved", &format!("workflow:{id}"), &json!({ "version": ver, "status": status }));
    get(conn, id)
}

pub fn set_status(conn: &Connection, id: i64, to: &str) -> Result<Value, String> {
    let (_, status, _, g) = load(conn, id)?;
    match to {
        "enabled" if !validate(&g).is_empty() => return Err("fix the workflow's problems before enabling it".into()),
        "enabled" if status == "archived" => return Err("an archived workflow cannot be enabled".into()),
        "enabled" | "disabled" | "archived" => {}
        _ => return Err("status must be enabled, disabled or archived".into()),
    }
    conn.execute("UPDATE workflows SET status = ?2, updated_utc = ?3 WHERE id = ?1", params![id, to, now()]).map_err(|e| e.to_string())?;
    let _ = crate::audit::record(conn, "user", &format!("workflow_{to}"), &format!("workflow:{id}"), &json!({}));
    get(conn, id)
}

/// Dry-run the current version against an event (a stored event id, or a made-up event body).
pub fn dry_run(conn: &Connection, id: i64, ev: &Value) -> Result<Value, String> {
    let (_, _, ver, g) = load(conn, id)?;
    let problems = validate(&g);
    if !problems.is_empty() {
        return Err(format!("the workflow has problems: {}", problems.join("; ")));
    }
    let (steps, _) = execute(conn, &g, ev, true);
    conn.execute("INSERT INTO workflow_runs (workflow_id, version, trigger_event_id, dry_run, status, steps, created_utc) VALUES (?1,?2,NULL,1,'dry_run',?3,?4)", params![id, ver, json!(steps).to_string(), now()]).map_err(|e| e.to_string())?;
    Ok(json!({ "workflow_id": id, "version": ver, "dry_run": true, "steps": steps, "note": "Nothing was changed." }))
}

/// Feed new events to enabled workflows. Returns research requests to queue. Safe to call every tick.
pub fn process(conn: &Connection) -> Vec<Value> {
    let cursor: i64 = crate::events::kv_get(conn, "workflows.cursor").and_then(|v| v.parse().ok()).unwrap_or_else(|| crate::events::latest_id(conn));
    let evs = crate::events::list(conn, cursor, None, 200).unwrap_or_default();
    if let Some(last) = evs.last().and_then(|e| e["id"].as_i64()) {
        crate::events::kv_set(conn, "workflows.cursor", &last.to_string());
    } else {
        crate::events::kv_set(conn, "workflows.cursor", &cursor.to_string());
        return vec![];
    }
    let wfs: Vec<i64> = conn.prepare("SELECT id FROM workflows WHERE status = 'enabled'").and_then(|mut st| st.query_map([], |r| r.get(0)).map(|m| m.filter_map(Result::ok).collect())).unwrap_or_default();
    let mut research = vec![];
    for ev in evs.iter().filter(|e| !SELF_EVENTS.contains(&e["type"].as_str().unwrap_or(""))) {
        for &w in &wfs {
            let Ok((_, _, ver, g)) = load(conn, w) else { continue };
            if g["trigger"]["event"] != ev["type"] || g["trigger"]["subject"].as_str().is_some_and(|s| !s.is_empty() && ev["subject"] != s) {
                continue;
            }
            let recent: i64 = conn.query_row("SELECT count(*) FROM workflow_runs WHERE workflow_id = ?1 AND dry_run = 0 AND created_utc >= ?2", params![w, (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)], |r| r.get(0)).unwrap_or(0);
            if recent >= RUNS_PER_HOUR {
                crate::events::notify(conn, "workflow", "Workflow rate limit reached", &format!("workflow #{w} ran {RUNS_PER_HOUR} times in the last hour; further runs are skipped"), Some(&format!("wf-rate:{w}")));
                continue;
            }
            // claim the (workflow, event) pair first: a second pass over the same event does nothing
            let n = conn.execute("INSERT OR IGNORE INTO workflow_runs (workflow_id, version, trigger_event_id, dry_run, status, steps, created_utc) VALUES (?1,?2,?3,0,'running','[]',?4)", params![w, ver, ev["id"].as_i64(), now()]).unwrap_or(0);
            if n == 0 {
                continue;
            }
            let run = conn.last_insert_rowid();
            crate::events::emit(conn, "WorkflowTriggered", &format!("workflow:{w}"), json!({ "workflow_id": w, "version": ver, "run_id": run, "event_id": ev["id"] }), ev["id"].as_i64().map(|i| format!("event:{i}")).as_deref(), Some(&format!("wf:{w}:{}", ev["id"])));
            let (steps, r) = execute(conn, &g, ev, false);
            research.extend(r);
            let _ = conn.execute("UPDATE workflow_runs SET status = 'done', steps = ?2 WHERE id = ?1", params![run, json!(steps).to_string()]);
        }
    }
    research
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(nodes: Value, edges: Value) -> Value {
        json!({ "trigger": { "event": "StrategyDriftDetected" }, "nodes": nodes, "edges": edges })
    }

    #[test]
    fn validation_refuses_orders_cycles_and_orphans() {
        let ok = g(json!([{ "id": "c", "type": "condition", "params": { "field": "payload.status", "op": "eq", "value": "degraded" } }, { "id": "n", "type": "notify", "params": { "title": "Drift" } }]), json!([["trigger", "c"], ["c", "n"]]));
        assert!(validate(&ok).is_empty(), "{:?}", validate(&ok));
        let order = g(json!([{ "id": "o", "type": "place_order", "params": {} }]), json!([["trigger", "o"]]));
        assert!(validate(&order)[0].contains("not allowed"));
        let cyc = g(json!([{ "id": "a", "type": "notify", "params": { "title": "x" } }, { "id": "b", "type": "notify", "params": { "title": "y" } }]), json!([["trigger", "a"], ["a", "b"], ["b", "a"]]));
        assert!(validate(&cyc).iter().any(|e| e.contains("cycle")));
        let orphan = g(json!([{ "id": "a", "type": "notify", "params": { "title": "x" } }]), json!([]));
        assert!(validate(&orphan).iter().any(|e| e.contains("not reachable")));
        let selfloop = json!({ "trigger": { "event": "WorkflowTriggered" }, "nodes": [{ "id": "a", "type": "notify", "params": { "title": "x" } }], "edges": [["trigger", "a"]] });
        assert!(!validate(&selfloop).is_empty());
    }

    #[test]
    fn dry_run_changes_nothing_and_live_runs_are_idempotent() {
        let c = crate::db::open(":memory:").unwrap();
        let graph = g(json!([{ "id": "c", "type": "condition", "params": { "field": "payload.status", "op": "eq", "value": "degraded" } }, { "id": "n", "type": "notify", "params": { "title": "Strategy drift" } }]), json!([["trigger", "c"], ["c", "n"]]));
        let w = save(&c, None, "Drift alert", &graph).unwrap();
        let id = w["id"].as_i64().unwrap();
        assert_eq!(w["status"], "validated");
        let ev = json!({ "id": 0, "type": "StrategyDriftDetected", "subject": "bot:1", "payload": { "status": "degraded" } });
        let d = dry_run(&c, id, &ev).unwrap();
        assert_eq!(d["steps"].as_array().unwrap().len(), 2);
        let notes: i64 = c.query_row("SELECT count(*) FROM notifications", [], |r| r.get(0)).unwrap();
        assert_eq!(notes, 0, "dry run notified nobody");
        // not enabled: events are ignored
        process(&c);
        crate::events::emit(&c, "StrategyDriftDetected", "bot:1", json!({ "status": "degraded" }), None, None);
        assert!(process(&c).is_empty());
        assert_eq!(c.query_row("SELECT count(*) FROM workflow_runs WHERE dry_run = 0", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        set_status(&c, id, "enabled").unwrap();
        let mark = crate::events::kv_get(&c, "workflows.cursor").unwrap();
        crate::events::emit(&c, "StrategyDriftDetected", "bot:1", json!({ "status": "degraded" }), None, None);
        crate::events::emit(&c, "StrategyDriftDetected", "bot:1", json!({ "status": "watch" }), None, None);
        process(&c);
        process(&c);
        assert_eq!(c.query_row("SELECT count(*) FROM workflow_runs WHERE dry_run = 0", [], |r| r.get::<_, i64>(0)).unwrap(), 2, "one run per event");
        assert_eq!(c.query_row("SELECT count(*) FROM notifications WHERE source = 'workflow'", [], |r| r.get::<_, i64>(0)).unwrap(), 1, "only the degraded event passed the condition");
        // replaying from an earlier cursor does not run twice
        crate::events::kv_set(&c, "workflows.cursor", &mark);
        process(&c);
        assert_eq!(c.query_row("SELECT count(*) FROM workflow_runs WHERE dry_run = 0", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
        // an edit needs a re-enable; an invalid graph cannot be enabled
        let bad = save(&c, Some(id), "Drift alert", &g(json!([{ "id": "o", "type": "place_order", "params": {} }]), json!([["trigger", "o"]]))).unwrap();
        assert_eq!(bad["status"], "draft");
        assert!(set_status(&c, id, "enabled").is_err());
    }
}
