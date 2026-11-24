//! A real daemon composes isolated launches, checks, corrections, PR retry/update and recurring recovery.
use super::*;
use serde_json::{Value, json};

struct DeliveryDaemon {
    repo: tempfile::TempDir,
    port: u16,
    daemon: Option<Child>,
}
impl Drop for DeliveryDaemon {
    fn drop(&mut self) {
        if let Some(mut daemon) = self.daemon.take() {
            stop_daemon(&mut daemon);
        }
    }
}
impl DeliveryDaemon {
    fn new() -> Self {
        let (repo, port, daemon, _) = stubbed_project();
        let mut fixture = Self {
            repo,
            port,
            daemon: Some(daemon),
        };
        fixture.git(&["branch", "-M", "main"]);
        fixture.git(&["config", "user.name", "Delivery eval"]);
        fixture.git(&["config", "user.email", "delivery@example.invalid"]);
        fixture.git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/owner/repo.git",
        ]);
        std::fs::write(
            fixture.repo.path().join(".git/info/exclude"),
            "stub-bin/\nstub-shell\nargv.txt\n",
        )
        .unwrap();
        fixture.install_stubs();
        fixture.restart();
        fixture
    }
    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(self.repo.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().into()
    }
    fn read(&self, path: &str) -> Value {
        let raw = get(self.port, path).unwrap_or_default();
        serde_json::from_str(&raw).unwrap_or_else(|_| panic!("GET {path}: {raw}"))
    }
    fn post(&self, path: &str, input: Value) -> Value {
        let raw = post_read(self.port, path, &input.to_string()).unwrap_or_default();
        serde_json::from_str(&raw).unwrap_or_else(|_| panic!("POST {path}: {raw}"))
    }
    fn wait(&self, path: &str, rule: impl Fn(&Value) -> bool) -> Value {
        let until = Instant::now() + Duration::from_secs(30);
        let mut last = Value::Null;
        while Instant::now() < until {
            last = self.read(path);
            if rule(&last) {
                return last;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("Timed out {path}: {last}");
    }
    fn receipt(&self, id: &str) -> Value {
        self.wait(&format!("/api/delivery/receipts/{id}"), |v| {
            [
                "completed",
                "failed",
                "failed_before_acceptance",
                "unconfirmed",
                "interrupted",
            ]
            .contains(&v["phase"].as_str().unwrap_or_default())
        })
    }
    fn restart(&mut self) {
        let mut daemon = self.daemon.take().unwrap();
        stop_daemon(&mut daemon);
        self.daemon = Some(with_window_token(
            Command::new(self.repo.path().join(".git/delivery-package/keel"))
                .arg("serve")
                .arg(self.repo.path())
                .args(["--port", &self.port.to_string(), "--no-open"])
                .env("GH_TOKEN", "controlled-test-token")
                .env("SHELL", self.repo.path().join("stub-shell"))
                .env("KEEL_PERMISSIONS_DIR", permission_test_storage()),
        ));
        wait_for_daemon(self.port);
    }
    fn install_stubs(&self) {
        let (bin, state, pushed, creates) = (
            self.repo.path().join("stub-bin"),
            self.repo.path().join(".git/delivery-host"),
            self.repo.path().join(".git/delivery-head"),
            self.repo.path().join(".git/delivery-creates"),
        );
        let git =
            String::from_utf8(Command::new("which").arg("git").output().unwrap().stdout).unwrap();
        let wrapper = format!(
            "#!/bin/sh\nif [ \"$1\" = push ]; then printf '%s' \"${{3%%:*}}\" > '{}'; exit 0; fi\nexec '{}' \"$@\"\n",
            pushed.display(),
            git.trim()
        );
        std::fs::write(bin.join("git"), wrapper).unwrap();
        chmod_x(&bin.join("git"));
        let script=r#"#!/usr/bin/env python3
import sys,json,pathlib,subprocess
a=sys.argv[1:]
state=pathlib.Path(__STATE__); pushed=pathlib.Path(__PUSHED__); creates=pathlib.Path(__CREATES__)
def result(value): print(json.dumps(value));sys.exit(0)
def current(): return pushed.read_text() if pushed.exists() else subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
if a[:2]==['repo','view']: result({'url':'https://github.com/owner/repo','nameWithOwner':'owner/repo'})
if a[:2]==['pr','list']:
 if not state.exists(): result([])
 result([{'url':'https://github.com/owner/repo/pull/1','headRepository':{'nameWithOwner':'owner/repo'},'headRepositoryOwner':{'login':'owner'},'headRefName':state.read_text(),'baseRefName':'main','headRefOid':current()}])
if a[:2]==['pr','create']:
 state.write_text(a[a.index('--head')+1]);creates.write_text(creates.read_text()+'x' if creates.exists() else 'x');print('https://github.com/owner/repo/pull/1');sys.exit(0)
if a[0]=='api':
 path=a[-1]
 if path=='repos/owner/repo': result({'full_name':'owner/repo','fork':False})
 if path=='repos/owner/repo/issues/1': result({'node_id':'I1','number':1,'html_url':'https://github.com/owner/repo/issues/1','title':'Preserve supervised delivery','body':'Make an isolated checked change.','state':'open','updated_at':'2026-11-23T00:00:00Z'})
print('Unexpected controlled GitHub call',file=sys.stderr);sys.exit(1)
"#.replace("__STATE__",&serde_json::to_string(&state).unwrap()).replace("__PUSHED__",&serde_json::to_string(&pushed).unwrap()).replace("__CREATES__",&serde_json::to_string(&creates).unwrap());
        std::fs::write(bin.join("gh"), script).unwrap();
        chmod_x(&bin.join("gh"));
        let argv = self.repo.path().join("argv.txt");
        let claude = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nprintf 'change\\n' >> delivery.txt\nsession=delivery-$(basename \"$PWD\")\nprintf '%s\\n' \"{{\\\"type\\\":\\\"system\\\",\\\"subtype\\\":\\\"init\\\",\\\"session_id\\\":\\\"$session\\\"}}\"\nprintf '%s\\n' \"{{\\\"type\\\":\\\"result\\\",\\\"subtype\\\":\\\"success\\\",\\\"session_id\\\":\\\"$session\\\",\\\"num_turns\\\":1,\\\"total_cost_usd\\\":0}}\"\n",
            argv.display()
        );
        std::fs::write(bin.join("claude"), claude).unwrap();
        chmod_x(&bin.join("claude"));
        std::fs::write(bin.join("codex"), "#!/bin/sh\nexit 0\n").unwrap();
        chmod_x(&bin.join("codex"));
        let package = self.repo.path().join(".git/delivery-package");
        std::fs::create_dir_all(package.join("runtime")).unwrap();
        std::fs::copy(env!("CARGO_BIN_EXE_keel"), package.join("keel")).unwrap();
        chmod_x(&package.join("keel"));
        std::fs::write(
            package.join("runtime/keel-runtime.mjs"),
            "// Controlled protocol fixture; the production runtime locator is exercised.\n",
        )
        .unwrap();
        let runtime = r#"#!/usr/bin/env python3
import sys,json,pathlib
query=None
def emit(event,data): print(json.dumps({'event':event,'data':data}),flush=True)
for line in sys.stdin:
 request=json.loads(line)
 if request['method']=='start':
  query=request['query'];emit('initialize',{'id':'controlled-launch'})
 elif request['method']=='initialized':
  if request.get('error'): emit('fatal',request['error']);emit('end',{'code':1});continue
  file=pathlib.Path('delivery.txt');file.write_text((file.read_text() if file.exists() else '')+'change\n')
  emit('msg',{'type':'system','subtype':'init','session_id':query.get('session') or 'delivery-'+query['lane']})
  emit('msg',{'type':'result','subtype':'success','result':'Controlled change completed','num_turns':1,'total_cost_usd':0})
  emit('end',{'code':0})
 elif request['method']=='interrupt': emit('end',{'code':-1})
 elif request['method']=='close': break
"#;
        std::fs::write(package.join("keel-node"), runtime).unwrap();
        chmod_x(&package.join("keel-node"));
    }
}

#[test]
fn eval_supervised_delivery_survives_retries_restart_and_recurrence() {
    let mut f = DeliveryDaemon::new();
    f.post("/api/permissions/trust", json!({"trusted":true}));
    let preview = f.post(
        "/api/delivery/issues/preview",
        json!({"url":"https://github.com/owner/repo/issues/1"}),
    );
    let input = json!({"receipt":"delivery-start","preview":preview["id"],"provider":"claude","base":"main"});
    let first = f.post("/api/delivery/tasks/start", input.clone());
    let second = f.post("/api/delivery/tasks/start", input);
    assert_eq!(first["task"], second["task"]);
    let task = first["task"].as_str().unwrap();
    let path = format!("/api/tasks/{task}");
    assert_eq!(f.receipt("delivery-start")["phase"], "completed");
    let mut view = f.wait(&path, |v| v["checks_passed"] == true);
    assert_eq!(
        f.read(&format!("{path}/recover"))["lane"],
        view["owner"]["lane"]
    );
    assert_eq!(
        view["record"]["source"]["snapshot"]["url"],
        "https://github.com/owner/repo/issues/1"
    );
    assert_ne!(view["owner"]["checkout"].as_str(), f.repo.path().to_str());
    for (batch, publication) in [
        ("delivery-batch-1", "delivery-publication-1"),
        ("delivery-batch-2", "delivery-publication-2"),
    ] {
        let correction=f.post(&format!("{path}/corrections"),json!({"revision":view["record"]["revision"],"body":"Apply the next supervised correction.","anchor":{"identity":view["identity"],"path":"delivery.txt","side":"new","first":1,"last":1,"quote":"change"}}));
        view = f.read(&path);
        let selected = serde_json::Map::from_iter([(
            correction["id"].as_str().unwrap().into(),
            correction["revision"].clone(),
        )]);
        f.post(
            &format!("{path}/batches"),
            json!({"receipt":batch,"owner":view["owner"],"selected":selected,"remote_head":null}),
        );
        assert_eq!(f.receipt(batch)["phase"], "completed");
        view = f.wait(&path, |v| v["checks_passed"] == true);
        assert_eq!(
            view["record"]["corrections"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["resolution"],
            "unresolved"
        );
        f.post(&format!("{path}/form/seed"), json!({}));
        view = f.read(&path);
        assert!(
            view["record"]["form"]["body"]
                .as_str()
                .unwrap()
                .contains("https://github.com/owner/repo/issues/1")
        );
        let publish = json!({"receipt":publication,"revision":view["record"]["revision"],"owner":view["owner"],"identity":view["identity"],"form":view["record"]["form"]});
        f.post(&format!("{path}/publication"), publish.clone());
        assert_eq!(f.receipt(publication)["phase"], "completed");
        f.post(&format!("{path}/publication"), publish);
        assert_eq!(f.receipt(publication)["phase"], "completed");
        view = f.read(&path);
        assert_eq!(
            view["record"]["publication"]["actual_head"],
            view["identity"]["head"]
        );
    }
    assert_eq!(
        std::fs::read_to_string(f.repo.path().join(".git/delivery-creates")).unwrap(),
        "x"
    );
    f.restart();
    view = f.read(&path);
    assert_eq!(view["record"]["corrections"].as_array().unwrap().len(), 2);
    assert!(view["checks_passed"].as_bool().unwrap());
    assert_eq!(
        view["record"]["publication"]["url"],
        "https://github.com/owner/repo/pull/1"
    );
    let source = json!({"lane":view["owner"]["lane"],"provider":view["owner"]["provider"],"session":view["owner"]["session"]});
    let saved = f.post("/api/task/save", source.clone());
    let continuation=f.post("/api/task/continue",json!({"source":source,"revision":saved["saved"].as_array().unwrap().last().unwrap()["revision"],"provider":"codex","receipt":"delivery-handoff"}));
    let handed = f.read(&path);
    assert_eq!(handed["owner"]["lane"], continuation["destination"]["lane"]);
    assert_eq!(handed["record"]["task"], task);
    assert_eq!(handed["record"]["corrections"].as_array().unwrap().len(), 2);
    assert_eq!(
        handed["record"]["publication"]["url"],
        "https://github.com/owner/repo/pull/1"
    );
    let schedule=f.post("/api/delivery/schedules",json!({"id":null,"revision":null,"name":"Supervised maintenance","prompt":"Make one checked maintenance change.","provider":"claude","base":"main","interval":60,"duration":60}));
    assert_eq!(schedule["enabled"], false);
    let action_path = format!(
        "/api/delivery/schedules/{}",
        schedule["id"].as_str().unwrap()
    );
    f.post(
        &action_path,
        json!({"action":"run_now","revision":schedule["revision"],"receipt":"delivery-occurrence"}),
    );
    let history = f.wait("/api/delivery/schedules", |v| {
        v["occurrences"].as_array().is_some_and(|all| {
            all.iter()
                .any(|o| o["id"] == "delivery-occurrence" && o["phase"] == "completed")
        })
    });
    let recurring = history["occurrences"][0]["task"].as_str().unwrap();
    assert_eq!(
        f.read(&format!("/api/tasks/{recurring}"))["record"]["source"]["kind"],
        "recurring"
    );
    let no_token = Command::new("curl")
        .args([
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            &format!("http://127.0.0.1:{}/api/delivery/schedules", f.port),
        ])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&no_token.stdout), "403");
}
