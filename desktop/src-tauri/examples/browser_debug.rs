//! Exercise the real embedded adapter in its own native window and scratch project profile.
#[cfg(target_os = "macos")]
#[path = "../src/browser.rs"]
mod browser;
#[cfg(target_os = "macos")]
#[path = "../src/preview.rs"]
mod preview;
#[cfg(target_os = "macos")]
#[path = "browser-debug/profile.rs"]
mod profile;
#[cfg(target_os = "macos")]
mod fixture {
    use super::{browser, profile};
    use serde_json::{Value, json};
    use tauri::Manager;
    const LANE: &str = "diagnostic-fixture";
    static STORE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    async fn raw(app: &tauri::AppHandle, project: &str, op: Value) -> Result<Value, String> {
        let output = browser::browser_act(app.clone(), LANE.into(), project.into(), op).await?;
        let value = serde_json::to_value(output).map_err(|e| e.to_string())?;
        ensure(
            value["identity"]["browser"] == "Inside Oya Factory",
            "native identity on every result",
        )?;
        if let Some(id) = value["identity"]["profile"]["id"].as_str() {
            if STORE.set(id.into()).is_ok() {
                println!("FIXTURE_STORE:{id}");
            }
        }
        Ok(value)
    }
    async fn act(app: &tauri::AppHandle, project: &str, op: Value) -> Result<Value, String> {
        let value = raw(app, project, op).await?;
        if value["ok"] == false {
            return Err(value["text"]
                .as_str()
                .unwrap_or("Native operation failed.")
                .into());
        }
        Ok(value)
    }
    fn ensure(condition: bool, why: &str) -> Result<(), String> {
        if condition { Ok(()) } else { Err(why.into()) }
    }
    fn element(read: &Value, frame: &str, label: &str) -> Result<u32, String> {
        let text = read["text"].as_str().unwrap_or("");
        let section = text
            .split(&format!("## Frame {frame}"))
            .nth(1)
            .ok_or("Missing frame section")?
            .split("## Frame ")
            .next()
            .unwrap_or("");
        section
            .lines()
            .find(|line| line.starts_with('[') && line.contains(label))
            .and_then(|line| line.strip_prefix('[')?.split(']').next()?.parse().ok())
            .ok_or("Missing element number".into())
    }
    async fn evaluate(
        app: &tauri::AppHandle,
        project: &str,
        frame: Option<&str>,
        expression: &str,
    ) -> Result<Value, String> {
        let mut op = json!({"op":"eval","expression":expression});
        if let Some(frame) = frame {
            op["frame"] = json!(frame);
        }
        Ok(act(app, project, op).await?["data"]["value"].clone())
    }
    async fn show(app: &tauri::AppHandle, shown: bool) -> Result<(), String> {
        let bounds = shown.then(|| {
            serde_json::from_value(json!({"x":0,"y":0,"width":900,"height":600})).unwrap()
        });
        browser::browser_place(app.clone(), LANE.into(), bounds).await
    }
    async fn native_focus(app: &tauri::AppHandle) -> Result<(usize, bool), String> {
        use objc2_app_kit::NSView;
        let view = app
            .get_webview(&format!("browser-{LANE}"))
            .ok_or("Fixture view closed")?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        view.with_webview(move |platform| unsafe {
            let view = &*(platform.inner() as *const NSView);
            let window = view.window().unwrap();
            let focus = window
                .firstResponder()
                .map(|object| &*object as *const _ as usize)
                .unwrap_or(0);
            let _ = tx.send((focus, window.isKeyWindow()));
        })
        .map_err(|e| e.to_string())?;
        tokio::time::timeout(std::time::Duration::from_secs(2), rx)
            .await
            .map_err(|_| "Native focus timeout")?
            .map_err(|_| "Fixture closed".into())
    }
    async fn state(app: &tauri::AppHandle, project: &str, frame: &str) -> Result<Value, String> {
        Ok(
            json!({"child":evaluate(app,project,Some(frame),"({clicks,count:events.length,scrollX,scrollY,focus:document.activeElement.id})").await?,"root":evaluate(app,project,None,"({scrollX,scrollY,focus:document.activeElement.id})").await?}),
        )
    }
    async fn rejection(
        app: &tauri::AppHandle,
        project: &str,
        frame: &str,
        reason: &str,
    ) -> Result<(), String> {
        let read = act(app, project, json!({"op":"read_frames"})).await?;
        let before = state(app, project, frame).await?;
        let focus = native_focus(app).await?;
        let rejected = raw(
            app,
            project,
            json!({"op":"click","target":element(&read,frame,"Frame button")?}),
        )
        .await?;
        ensure(
            rejected["ok"] == false && rejected["outcome"] == "native_input_unavailable",
            "explicit native presentation error",
        )?;
        ensure(
            rejected["data"]["reason"] == reason && rejected["data"]["dispatched"] == json!([]),
            "zero native dispatch on rejection",
        )?;
        ensure(
            state(app, project, frame).await? == before,
            "no scrolling, DOM focus, movement or activation on rejection",
        )?;
        ensure(
            native_focus(app).await? == focus,
            "no native focus change on rejection",
        )
    }
    async fn click(
        app: &tauri::AppHandle,
        project: &str,
        frame: &str,
        expected: u32,
    ) -> Result<(), String> {
        let read = act(app, project, json!({"op":"read_frames"})).await?;
        let clicked = act(
            app,
            project,
            json!({"op":"click","target":element(&read,frame,"Frame button")?}),
        )
        .await?;
        ensure(
            clicked["data"]["input"] == "native_mouse",
            "native method disclosed",
        )?;
        let observed = evaluate(app, project, Some(frame), "({clicks,events})").await?;
        ensure(
            observed["clicks"] == expected,
            "native click activated exact frame",
        )?;
        ensure(
            observed["events"]
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e["trusted"] == true),
            "every observed native event is trusted",
        )
    }
    async fn fixed(app: &tauri::AppHandle, project: &str, frame: &str) -> Result<(), String> {
        act(
            app,
            project,
            json!({"op":"viewport","settings":{"width":375,"height":812}}),
        )
        .await?;
        ensure(
            evaluate(app, project, None, "({w:innerWidth,h:innerHeight})").await?
                == json!({"w":375,"h":812}),
            "375 by 812 CSS viewport",
        )?;
        let shot = act(app, project, json!({"op":"screenshot"})).await?;
        let dpr = shot["identity"]["dpr"]
            .as_f64()
            .ok_or("Missing measured DPR")?;
        ensure(
            shot["data"]["image"]["width"].as_f64() == Some(375.0 * dpr)
                && shot["data"]["image"]["height"].as_f64() == Some(812.0 * dpr),
            "fixed screenshot physical dimensions",
        )?;
        rejection(app, project, frame, "fixed_snapshot").await?;
        let read = act(app, project, json!({"op":"read_frames"})).await?;
        let typed = act(
            app,
            project,
            json!({"op":"type","target":element(&read,frame,"Name")?,"text":"Ada"}),
        )
        .await?;
        ensure(
            typed["data"]["input"] == "dom_value"
                && evaluate(
                    app,
                    project,
                    Some(frame),
                    "document.getElementById('name').value",
                )
                .await?
                    == "Ada",
            "fixed inspection retains disclosed DOM typing",
        )
    }
    async fn baseline_read(
        app: &tauri::AppHandle,
        project: &str,
        frame: &str,
    ) -> Result<(), String> {
        let html = r#"<label for="n">First name</label><input id="n"><span id="l">Work email</span><input aria-labelledby="l"><details><summary>Advanced</summary></details><div role="option" tabindex="0">Inbox</div><div id="editor" contenteditable="plaintext-only">Draft</div><select aria-label="Plan"><option>Free</option><option>Team</option></select>"#;
        evaluate(
            app,
            project,
            Some(frame),
            &format!(
                "document.body.insertAdjacentHTML('beforeend',{}),true",
                serde_json::to_string(html).unwrap()
            ),
        )
        .await?;
        let read = act(app, project, json!({"op":"read_frames"})).await?;
        for name in [
            "First name",
            "Work email",
            "Advanced",
            "Inbox",
            "Draft",
            "Plan",
        ] {
            ensure(
                element(&read, frame, name).is_ok(),
                "baseline controls retain actionable names",
            )?;
        }
        let text = read["text"].as_str().unwrap_or("");
        ensure(
            text.contains("Title: Child form") && text.contains("\"options\":[\"Free\",\"Team\"]"),
            "frame title and option facts retained",
        )?;
        act(
            app,
            project,
            json!({"op":"type","target":element(&read,frame,"Draft")?,"text":"Updated draft"}),
        )
        .await?;
        ensure(
            evaluate(
                app,
                project,
                Some(frame),
                "document.getElementById('editor').textContent",
            )
            .await?
                == "Updated draft",
            "plaintext-only private typing capability",
        )
    }
    async fn run(app: &tauri::AppHandle, url: &str, project: &str) -> Result<(), String> {
        act(app, project, json!({"op":"open","url":url})).await?;
        show(app, true).await?;
        let read = act(app, project, json!({"op":"read_frames"})).await?;
        ensure(read["data"]["read"] == 3, "independent cross-origin reads")?;
        let hit = act(app, project, json!({"op":"hit_test","x":120,"y":105})).await?;
        ensure(
            hit["data"]["element"]["id"] == "target",
            "cross-frame real hit",
        )?;
        let frame = hit["data"]["frame"]
            .as_str()
            .ok_or("Missing exact frame")?
            .to_string();
        click(app, project, &frame, 1).await?;
        show(app, false).await?;
        rejection(app, project, &frame, "parked").await?;
        show(app, true).await?;
        fixed(app, project, &frame).await?;
        act(
            app,
            project,
            json!({"op":"viewport","settings":{"reset":true}}),
        )
        .await?;
        click(app, project, &frame, 2).await?;
        overlay(app, project, &frame).await?;
        baseline_read(app, project, &frame).await?;
        println!(
            "PASS embedded public adapter: frame read/eval/hit/native click/type,375px screenshot,zero-effect snapshot/hidden refusals,reset recovery,overlay,identity"
        );
        Ok(())
    }
    async fn overlay(app: &tauri::AppHandle, project: &str, frame: &str) -> Result<(), String> {
        evaluate(app,project,None,"(()=>{const n=document.createElement('div');n.id='cover';n.style.cssText='position:fixed;inset:0;background:white;z-index:999';document.body.append(n);return true})()").await?;
        let hit = act(app, project, json!({"op":"hit_test","x":120,"y":105})).await?;
        ensure(
            hit["data"]["element"]["id"] == "cover",
            "overlay named by real hit",
        )?;
        let read = act(app, project, json!({"op":"read_frames"})).await?;
        let blocked = raw(
            app,
            project,
            json!({"op":"click","target":element(&read,frame,"Frame button")?}),
        )
        .await?;
        ensure(
            blocked["ok"] == false && blocked["text"].as_str().unwrap_or("").contains("covered"),
            "covered target refused",
        )?;
        ensure(
            evaluate(app, project, Some(frame), "clicks").await? == 2,
            "covered click did not activate",
        )
    }
    /// Share one generated app context so the fixture embeds exactly one Info.plist.
    fn context() -> tauri::Context<tauri::Wry> {
        tauri::generate_context!("examples/browser-debug/tauri.conf.json")
    }
    /// Cleanup starts only after the testing process releases all WebKit objects.
    fn cleanup(identifier: String) {
        tauri::Builder::default()
            .setup(move |app| {
                tauri::WebviewWindowBuilder::new(
                    app,
                    "cleanup",
                    tauri::WebviewUrl::External("about:blank".parse().unwrap()),
                )
                .title("Diagnostic fixture cleanup")
                .incognito(true)
                .visible(false)
                .build()?;
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let result = profile::remove(&handle, identifier).await;
                    if let Err(why) = &result {
                        eprintln!("FAIL cleanup: {why}");
                    }
                    handle.exit(if result.is_ok() { 0 } else { 1 });
                });
                Ok(())
            })
            .run(context())
            .expect("fixture store cleaner");
    }
    pub fn start() {
        let args: Vec<String> = std::env::args().collect();
        if args.get(1).is_some_and(|arg| arg == "--remove-store") {
            return cleanup(args.get(2).expect("owned fixture UUID").clone());
        }
        let url = args.get(1).expect("fixture URL").clone();
        let project = args.get(2).expect("scratch project").clone();
        tauri::Builder::default()
            .setup(move |app| {
                tauri::WindowBuilder::new(app, "main")
                    .title("Oya Factory diagnostic fixture")
                    .inner_size(900.0, 650.0)
                    .build()?;
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let result = run(&handle, &url, &project).await;
                    if let Err(why) = &result {
                        eprintln!("FAIL {why}");
                    }
                    let _ = browser::browser_close(handle.clone(), LANE.into()).await;
                    handle.exit(if result.is_ok() { 0 } else { 1 });
                });
                Ok(())
            })
            .run(context())
            .expect("native fixture application");
    }
}
#[cfg(target_os = "macos")]
fn main() {
    fixture::start();
}
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("Embedded diagnostic fixture requires macOS WebKit.");
    std::process::exit(1);
}
