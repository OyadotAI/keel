//! Local Oya operations use only this binding's owned tab and never replay uncertain effects.
use super::{Op, connection::Client};
use serde_json::json;
#[derive(Default)]
pub(super) struct Page {
    client: Option<Client>,
    tab: Option<u64>,
    pub url: Option<String>,
    analyzed: bool,
}
impl Page {
    pub fn reset(&mut self) {
        if let Some(client) = self.client.take() {
            tokio::spawn(client.close());
        }
        self.tab = None;
        self.url = None;
        self.analyzed = false;
    }
    pub fn connected(&self) -> bool {
        self.client.is_some()
    }
    pub async fn connect(&mut self, client: Client) -> Result<(), String> {
        let mut client = client;
        self.tab = None;
        self.analyzed = false;
        client.initialize().await?;
        self.client = Some(client);
        Ok(())
    }
    pub async fn run(&mut self, op: Op) -> Result<String, String> {
        let result = self.perform(op).await;
        match &result {
            // Its tab ids belong to an Oya that is gone: start clean, and the next step reconnects.
            Err(why) if why == super::connection::RESTARTED => self.reset(),
            Err(_) => self.analyzed = false,
            Ok(_) => {}
        }
        result
    }
    async fn perform(&mut self, op: Op) -> Result<String, String> {
        let client=self.client.as_mut().ok_or("Local Oya is not connected. Use Retry connection in Browser settings; Keel did not switch browsers.")?;
        if let Op::Open { url } = op {
            self.analyzed = false;
            if let Some(tab) = self.tab {
                client.tool("switch_tab", json!({"tab_id":tab})).await?;
                let result = client.tool("navigate", json!({"url":url})).await?;
                self.url = Some(url);
                return Ok(result);
            }
            let result = client.tool("open_tab", json!({"url":url})).await?;
            let value: serde_json::Value = serde_json::from_str(&result)
                .map_err(|_| "Oya did not return an owned tab identity.")?;
            self.tab = Some(
                value["tab_id"]
                    .as_u64()
                    .ok_or("Oya did not return an owned tab identity.")?,
            );
            self.url = Some(url);
            return Ok(result);
        }
        let tab=self.tab.ok_or("Open a page with browser_open before using local Oya. Personal tabs are not available to this lane.")?;
        client.tool("switch_tab", json!({"tab_id":tab})).await?;
        match op {
            Op::Read => {
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
    pub async fn show(&mut self) -> Result<String, String> {
        let tab = self.tab.ok_or("This lane has no owned Oya tab yet.")?;
        self.client
            .as_mut()
            .ok_or("Local Oya is disconnected.")?
            .tool("switch_tab", json!({"tab_id":tab}))
            .await
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
mod tests {
    use super::*;
    #[test]
    fn typing_keeps_the_read_unless_it_submits() {
        assert!(read_survives_typing(false));
        assert!(!read_survives_typing(true));
    }
    #[test]
    fn element_actions_require_a_fresh_page_analysis() {
        assert!(require_analysis(false).is_err());
        assert!(require_analysis(true).is_ok());
    }
    #[tokio::test]
    async fn disconnected_local_browser_never_uses_an_embedded_or_personal_tab() {
        let mut page = Page::default();
        assert!(
            page.run(Op::Read)
                .await
                .unwrap_err()
                .contains("not connected")
        );
        assert!(page.show().await.unwrap_err().contains("no owned"));
    }
    /// Live: fills a form from one read in the installed Oya, including a field typed by its old
    /// id after a select revealed a new field above it. Needs Oya running and `KEEL_LIVE_FORM`
    /// naming a page shaped like the one this asserts on.
    #[tokio::test]
    #[ignore = "needs a running Oya Browser"]
    async fn live_form_fills_from_one_read() {
        let url = std::env::var("KEEL_LIVE_FORM").expect("KEEL_LIVE_FORM");
        let installation = super::super::detection::installed()
            .unwrap()
            .expect("Oya installed");
        let project = camino::Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let connection = super::super::connection::discover(&project, &installation).unwrap();
        let mut page = Page::default();
        page.connect(Client::new(connection).unwrap())
            .await
            .unwrap();
        page.run(Op::Open { url }).await.unwrap();
        page.run(Op::Read).await.unwrap();
        let fields = [
            (1, "Ada"),
            (2, "Lovelace"),
            (3, "ada@example.com"),
            (4, "5550100"),
            (5, "Business"),
            (6, "London"),
        ];
        for (target, text) in fields {
            let typed = page
                .run(Op::Type {
                    target,
                    text: text.into(),
                    submit: false,
                })
                .await;
            assert!(typed.is_ok(), "field {target}: {typed:?}");
        }
        let after = page.run(Op::Read).await.unwrap();
        println!("{after}");
        for want in [
            r#""First name" = "Ada""#,
            r#""Phone" = "5550100""#,
            r#""Account type" = "Business""#,
            r#""City" = "London""#,
        ] {
            assert!(after.contains(want), "missing {want}");
        }
        assert!(
            after.contains(r#"input:text "Company"]"#),
            "Company stays empty"
        );
        let tab = page.tab.unwrap();
        let _ = page
            .client
            .as_mut()
            .unwrap()
            .tool("close_tab", json!({"tab_id": tab}))
            .await;
    }
}
