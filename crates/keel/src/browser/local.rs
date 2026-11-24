//! Local Oya operations use only this binding's owned tab and never replay uncertain effects.
use super::{Op, Output, connection::Client};
use serde_json::json;
const TAB_CLOSED: &str = "This lane's tab was closed. Open a page with browser_open to continue.";

enum Selection {
    Active(String),
    Closed,
}
impl Selection {
    fn text(self) -> Result<String, String> {
        match self {
            Self::Active(text) => Ok(text),
            Self::Closed => Err(TAB_CLOSED.into()),
        }
    }
}
#[derive(Default)]
pub(super) struct Page {
    client: Option<Client>,
    tab: Option<u64>,
    pub url: Option<String>,
    analyzed: bool,
    framed: bool,
    cleanup_runtime: Option<tokio::runtime::Handle>,
}
impl Page {
    /// Retire the connection and its page references together after replacement or restart.
    pub fn reset(&mut self) {
        if let Some(client) = self.client.take()
            && let Some(runtime) = self.cleanup_runtime.take()
        {
            runtime.spawn(client.close());
        }
        self.clear_tab();
    }
    /// Forget only the closed page; the authenticated session still owns any replacement tab.
    fn clear_tab(&mut self) {
        self.tab = None;
        self.url = None;
        self.analyzed = false;
        self.framed = false;
    }
    /// Every page path selects its owned tab before acting, so closure never targets another tab.
    async fn switch_tab(&mut self, tab: u64) -> Result<Selection, String> {
        let result = self
            .client
            .as_mut()
            .ok_or("Local Oya is disconnected.")?
            .tool("switch_tab", json!({"tab_id":tab}))
            .await;
        match result {
            Err(why) if tab_gone(&why, tab) => {
                self.clear_tab();
                Ok(Selection::Closed)
            }
            result => result.map(Selection::Active),
        }
    }
    pub fn connected(&self) -> bool {
        self.client.is_some()
    }
    pub async fn connect(&mut self, client: Client) -> Result<(), String> {
        self.reset();
        let mut client = client;
        let runtime = tokio::runtime::Handle::current();
        if let Err(why) = client.initialize().await {
            client.close().await;
            return Err(why);
        }
        self.cleanup_runtime = Some(runtime);
        self.client = Some(client);
        Ok(())
    }
    pub async fn run(&mut self, op: Op) -> Result<Output, String> {
        let before = self.observe_identity().await;
        let result = self.run_step(op).await;
        let observed = self.observe_identity().await.or(before);
        match result {
            Ok(mut output) => {
                if output.identity.is_none() {
                    output.identity = observed;
                }
                Ok(output)
            }
            Err(text) if observed.is_some() => Ok(Output {
                text,
                identity: observed,
                ok: Some(false),
                ..Default::default()
            }),
            Err(why) => Err(why),
        }
    }
    /// Metadata runs only after Keel's lane gate, against this session's selected owned tab.
    async fn observe_identity(&mut self) -> Option<serde_json::Value> {
        let tab = self.tab?;
        if !self.client.as_ref()?.supports("browser_identity") {
            return None;
        }
        self.switch_tab(tab).await.ok()?.text().ok()?;
        let reply = self
            .client
            .as_mut()?
            .diagnostic("browser_identity", json!({}))
            .await
            .ok()?;
        reply.identity
    }
    /// Stateful dispatch stays separate from provenance collection so errors preserve observed metadata.
    async fn run_step(&mut self, op: Op) -> Result<Output, String> {
        let result = match op {
            Op::Read
                if self
                    .client
                    .as_ref()
                    .is_some_and(|client| client.supports("browser_read_frames")) =>
            {
                self.debug(op).await
            }
            Op::ReadFrames | Op::Eval { .. } | Op::HitTest { .. } | Op::Click { .. } => {
                self.debug(op).await
            }
            Op::Type { .. } if self.framed => self.debug(op).await,
            Op::Screenshot => self.capture().await,
            op => self.perform(op).await.map(Output::text),
        };
        match &result {
            // Its tab ids belong to an Oya that is gone: start clean, and the next step reconnects.
            Err(why) if why == super::connection::RESTARTED => self.reset(),
            Err(_) => self.analyzed = false,
            Ok(_) => {}
        }
        result
    }
    /// New capabilities never fall back to an older Oya tool with different input or frame semantics.
    async fn debug(&mut self, op: Op) -> Result<Output, String> {
        let tab = self
            .tab
            .ok_or("Open a page with browser_open before debugging the owned tab.")?;
        self.switch_tab(tab).await?.text()?;
        let (name, args) = match op {
            Op::Read | Op::ReadFrames => {
                self.framed = true;
                self.analyzed = true;
                (
                    "browser_read_frames",
                    json!({"frames":matches!(op,Op::ReadFrames)}),
                )
            }
            Op::Eval { expression, frame } => {
                self.analyzed = false;
                (
                    "browser_eval",
                    match frame {
                        Some(frame) => json!({"expression":expression,"frame":frame}),
                        None => json!({"expression":expression}),
                    },
                )
            }
            Op::HitTest { x, y } => ("browser_hit_test", json!({"x":x,"y":y})),
            Op::Click { target } => {
                require_analysis(self.analyzed)?;
                self.analyzed = false;
                (
                    "browser_native_click",
                    json!({"element":target,"framed":self.framed}),
                )
            }
            Op::Type {
                target,
                text,
                submit,
            } => {
                require_analysis(self.analyzed)?;
                self.analyzed = read_survives_typing(submit);
                (
                    "browser_frame_type",
                    json!({"element":target,"text":text,"submit":submit}),
                )
            }
            _ => return Err("Not a browser diagnostic operation.".into()),
        };
        let result = self
            .client
            .as_mut()
            .ok_or("Local Oya is disconnected.")?
            .diagnostic(name, args)
            .await?;
        if result.ok == Some(false) {
            self.analyzed = false;
        }
        Ok(result)
    }
    async fn perform(&mut self, op: Op) -> Result<String, String> {
        if self.client.is_none() {
            return Err("Local Oya is not connected. Use Retry connection in Browser settings; Keel did not switch browsers.".into());
        }
        if let Op::Open { url } = op {
            return self.open(url).await;
        }
        let tab=self.tab.ok_or("Open a page with browser_open before using local Oya. Personal tabs are not available to this lane.")?;
        self.switch_tab(tab).await?.text()?;
        let client = self.client.as_mut().unwrap();
        match op {
            Op::Viewport { settings } => {
                if !client.supports("viewport") {
                    return Err("This Oya version cannot set the viewport. Update Oya and retry; Keel did not switch browsers.".into());
                }
                let result = client
                    .tool(
                        "viewport",
                        serde_json::to_value(settings).map_err(|e| e.to_string())?,
                    )
                    .await?;
                self.analyzed = false;
                Ok(result)
            }
            Op::Screenshot => unreachable!(),
            Op::ReadFrames | Op::Eval { .. } | Op::HitTest { .. } => Err("This Oya version does not expose the requested debugging tool. Update Oya; no other browser was used.".into()),
            Op::Read => {
                self.framed = false;
                let text = client
                    .tool("analyze_page", json!({"format":"markdown"}))
                    .await?;
                self.analyzed = true;
                Ok(text)
            }
            Op::Click { target } => {
                require_analysis(self.analyzed)?;
                self.analyzed = false;
                client.tool("click", json!({"element_id":target})).await
            }
            Op::Type {
                target,
                text,
                submit,
            } => {
                require_analysis(self.analyzed)?;
                self.analyzed = read_survives_typing(submit);
                let typed = client
                    .tool("type", json!({"element_id":target,"text":text}))
                    .await?;
                if submit {
                    client.tool("press_key", json!({"key":"Enter"})).await
                } else {
                    Ok(typed)
                }
            }
            Op::Key { key } => {
                self.analyzed = false;
                client.tool("press_key", json!({"key":key})).await
            }
            Op::Scroll { direction } => {
                self.analyzed = false;
                client.tool("scroll", json!({"direction":direction})).await
            }
            Op::Back => {
                self.analyzed = false;
                client.tool("go_back", json!({})).await
            }
            Op::Open { .. } => unreachable!(),
        }
    }
    /// Opening may replace a known-closed tab, but never retries an uncertain navigation.
    async fn open(&mut self, url: String) -> Result<String, String> {
        self.analyzed = false;
        if let Some(tab) = self.tab
            && matches!(self.switch_tab(tab).await?, Selection::Active(_))
        {
            let result = self
                .client
                .as_mut()
                .unwrap()
                .tool("navigate", json!({"url":url}))
                .await?;
            self.url = Some(url);
            return Ok(result);
        }
        let result = self
            .client
            .as_mut()
            .unwrap()
            .tool("open_tab", json!({"url":url}))
            .await?;
        let value: serde_json::Value = serde_json::from_str(&result)
            .map_err(|_| "Oya did not return an owned tab identity.")?;
        self.tab = Some(
            value["tab_id"]
                .as_u64()
                .ok_or("Oya did not return an owned tab identity.")?,
        );
        self.url = Some(url);
        Ok(result)
    }
    /// Captures only this session's owned tab, retaining element references on success.
    async fn capture(&mut self) -> Result<Output, String> {
        let tab = self
            .tab
            .ok_or("No page is open in this lane. Open one with browser_open, then retry.")?;
        let client = self
            .client
            .as_ref()
            .ok_or("Local Oya is not connected. Retry connection in Browser settings.")?;
        if !client.supports("screenshot") {
            return Err(
                "This Oya version cannot capture screenshots. Update Oya and retry.".into(),
            );
        }
        self.switch_tab(tab).await?.text()?;
        self.client.as_mut().unwrap().capture().await
    }
    pub async fn show(&mut self) -> Result<String, String> {
        let tab = self.tab.ok_or("This lane has no owned Oya tab yet.")?;
        self.switch_tab(tab).await?.text()
    }
}

/// Oya's switch_tab exposes no tab-specific error code, only this exact text. Match the owned
/// id and the whole reply so timeouts, ownership refusals and other tab ids retain their state.
fn tab_gone(why: &str, tab: u64) -> bool {
    why == format!("Tab {tab} not found")
}
/// Retirement/replacement can drop a binding on a worker thread; its connected runtime owns cleanup.
impl Drop for Page {
    fn drop(&mut self) {
        self.reset();
    }
}

fn require_analysis(analyzed: bool) -> Result<(), String> {
    if analyzed {
        Ok(())
    } else {
        Err(
            "Read the current owned page with browser_read before using an element reference."
                .into(),
        )
    }
}

/// Whether the last read's element ids still hold after typing. Oya's ids hold until the next
/// read (only a read numbers the page afresh), so a form fills from one read; forcing a read per
/// field made every form a full page analysis per field. Enter can post the form, so a submit
/// still needs a fresh read.
fn read_survives_typing(submit: bool) -> bool {
    !submit
}

#[cfg(test)]
#[path = "local_tests.rs"]
mod tests;
